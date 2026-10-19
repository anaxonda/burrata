#!/usr/bin/env python3
"""Compare explicit public video excerpts without retaining streams or audio.

Uses the bundled yt-dlp/mweb format policy and bundled BgUtils provider in a
private loopback process. This is reference analysis, not a production latency
benchmark. PCM and unredacted extraction metadata remain temporary.
"""
import argparse
import hashlib
import json
import math
import os
from pathlib import Path
import re
import signal
import socket
import subprocess
import tempfile
import time
from urllib.request import urlopen
import wave

from playback_reliability import terminate_owned_tree

FORMAT = 'bestaudio[ext=m4a][protocol^=http][protocol!*=dash]/bestaudio[protocol^=http][protocol!*=dash]/bestaudio/worst[ext=mp4][acodec!=none][vcodec!=none][protocol^=http][protocol!*=dash]'


class Failure(Exception):
    pass


def bounded(command, env, deadline):
    remaining = deadline - time.monotonic()
    if remaining <= 0:
        raise Failure('TIMEOUT')
    child = subprocess.Popen(command, stdout=subprocess.PIPE, stderr=subprocess.PIPE,
                             stdin=subprocess.DEVNULL, env=env, start_new_session=True)
    try:
        output, errors = child.communicate(timeout=remaining)
    except subprocess.TimeoutExpired:
        owned = terminate_owned_tree(child.pid, signal.SIGTERM)
        try:
            child.communicate(timeout=.5)
        except subprocess.TimeoutExpired:
            terminate_owned_tree(child.pid, signal.SIGKILL, owned)
            child.communicate()
        raise Failure('TIMEOUT')
    if child.returncode:
        if re.search(rb'(?:HTTP[^\n]{0,40}|Server returned[^\n]{0,20})(?:403|410)', errors, re.IGNORECASE):
            raise Failure('STREAM_DENIED')
        raise Failure('TOOL_FAILED')
    if len(output) > 8 * 1024 * 1024:
        raise Failure('OUTPUT_LIMIT')
    return output


def text(value, length=300):
    return value[:length] if isinstance(value, str) and '://' not in value else None


def public_identity(info):
    return dict(videoId=info.get('id'), title=text(info.get('title')),
                durationSeconds=info.get('duration'), uploader=text(info.get('uploader')),
                channel=text(info.get('channel')), channelId=text(info.get('channel_id')),
                channelIsVerified=info.get('channel_is_verified'),
                track=text(info.get('track')), album=text(info.get('album')),
                artist=text(info.get('artist')),
                artists=[text(value, 120) for value in (info.get('artists') or [])[:16]],
                releaseYear=info.get('release_year'), formatId=text(info.get('format_id')),
                codec=text(info.get('acodec')), extractionProfile='bundled-production-mweb-progressive-format-policy')


def fingerprint_alignment(left, right, max_shift=40):
    best = None
    for shift in range(-max_shift, max_shift + 1):
        a, b = left[max(shift, 0):], right[max(-shift, 0):]
        overlap = min(len(a), len(b))
        if overlap < min(len(left), len(right)) * .65:
            continue
        different = sum(((int(x) ^ int(y)) & 0xffffffff).bit_count() for x, y in zip(a[:overlap], b[:overlap]))
        row = dict(shiftFrames=shift, overlapFrames=overlap, differingBits=different,
                   bitAgreement=1 - different / (overlap * 32))
        if best is None or row['bitAgreement'] > best['bitAgreement']:
            best = row
    return best


def sample_alignment(left_path, right_path, max_offset_seconds=5, rate_search=False):
    # Optional experiment-only installed analysis package; no dependency is added.
    import numpy as np
    def read(path):
        with wave.open(str(path)) as stream:
            rate = stream.getframerate()
            return np.frombuffer(stream.readframes(stream.getnframes()), dtype='<i2').astype(np.float64), rate
    x, rate = read(left_path)
    y, right_rate = read(right_path)
    if rate != right_rate:
        raise Failure('PCM_RATE_MISMATCH')
    x -= x.mean(); y -= y.mean()
    size = 1 << (len(x) + math.ceil(len(y)/.975) - 1).bit_length()
    x_fft = np.fft.rfft(x, size)
    shifts = np.arange(-int(rate * max_offset_seconds), int(rate * max_offset_seconds) + 1)
    a_start, b_start = np.maximum(shifts, 0), np.maximum(-shifts, 0)
    def prefix(values):
        return np.concatenate(([0.], np.cumsum(values)))
    sx, xx = prefix(x), prefix(x*x)
    def score(values, ratio):
        correlation = np.fft.irfft(x_fft * np.conj(np.fft.rfft(values, size)), size)
        counts = np.minimum(len(x) - a_start, len(values) - b_start)
        a_end, b_end = a_start + counts, b_start + counts
        sy, yy = prefix(values), prefix(values*values)
        ax, by = sx[a_end] - sx[a_start], sy[b_end] - sy[b_start]
        numerator = correlation[shifts % size] - ax * by / counts
        denominator = np.sqrt(np.maximum(0, (xx[a_end]-xx[a_start]-ax*ax/counts) * (yy[b_end]-yy[b_start]-by*by/counts)))
        normalized = np.divide(numerator, denominator, out=np.zeros_like(numerator), where=denominator>0)
        index = int(np.argmax(np.abs(normalized)))
        return dict(normalizedCorrelation=float(normalized[index]), absoluteCorrelation=float(abs(normalized[index])),
                    shiftSamples=int(shifts[index]), shiftSeconds=float(shifts[index]/rate),
                    overlapSeconds=float(counts[index]/rate), sampleRate=rate, rightSampleStep=ratio,
                    method='FFT normalized cross-correlation with overlap centering; uniform resampling when rightSampleStep differs from1', numpyVersion=np.__version__)
    best = score(y, 1.)
    if rate_search:
        deadline = time.monotonic()+30
        positions = np.arange(len(y))
        tested = 1
        for step in [.0005, .000025]:
            center, radius = (1., .025) if step == .0005 else (best['rightSampleStep'], .0005)
            for ratio in np.arange(center-radius, center+radius+step/2, step):
                if time.monotonic() >= deadline:
                    best['rateSearchTruncated'] = True
                    break
                values = np.interp(np.arange(0, len(y), ratio), positions, y)
                candidate = score(values, float(ratio)); tested += 1
                if candidate['absoluteCorrelation'] > best['absoluteCorrelation']:
                    best = candidate
        best['rateHypothesesTested'] = tested
    return best


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--resources', type=lambda value: Path(value).resolve(), required=True)
    parser.add_argument('--left-video', required=True)
    parser.add_argument('--right-video', required=True)
    parser.add_argument('--seconds', type=int, default=30)
    parser.add_argument('--position', type=float, default=0)
    parser.add_argument('--budget-seconds', type=int, default=20)
    parser.add_argument('--output', type=Path, required=True)
    args = parser.parse_args()
    if not all(re.fullmatch(r'[A-Za-z0-9_-]{11}', value) for value in [args.left_video, args.right_video]):
        parser.error('explicit 11-character public video IDs required')
    if not 10 <= args.seconds <= 30 or not 1 <= args.budget_seconds <= 120 or not math.isfinite(args.position) or args.position < 0:
        parser.error('seconds must be10..30, budget1..120, position finite and nonnegative')
    root = args.resources
    candidates = list((root / 'tools').glob('*/yt-dlp'))
    if not candidates:
        parser.error('bundled runtime directory unavailable')
    tools = candidates[0].parent
    provider = root / 'youtube-support/bgutil'
    report = dict(schema='burrata.public-excerpt-equivalence/v1', qualification=False,
                  leftVideoId=args.left_video, rightVideoId=args.right_video,
                  excerptSeconds=args.seconds, positionSeconds=args.position, perVideoBudgetSeconds=args.budget_seconds,
                  scope='explicit fixed-video reference analysis; no search selection or playback latency claim',
                  limitations=['Excerpt correspondence does not establish complete-track edit equivalence or album mastering.',
                               'Provider artist/album metadata is evidence, not independent acoustic identification.',
                               'PCM and unredacted extraction JSON are temporary and removed; no stream URLs are reported.'],
                  sources=[], failures=[])
    stage = 'helper-startup'
    helper = None
    with tempfile.TemporaryDirectory(prefix='burrata-public-equivalence-') as directory:
        scratch = Path(directory)
        env = dict(os.environ, XDG_CACHE_HOME=str(scratch/'cache'), XDG_CONFIG_HOME=str(scratch/'config'), XDG_DATA_HOME=str(scratch/'data'))
        try:
            with socket.socket() as reserved:
                reserved.bind(('127.0.0.1', 0)); port = reserved.getsockname()[1]
            helper = subprocess.Popen([str(tools/'node'), str(provider/'server/build/main.js'), '--host', '127.0.0.1', '--port', str(port)],
                                      stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, env=env, start_new_session=True)
            base = f'http://127.0.0.1:{port}'
            startup_deadline = time.monotonic() + args.budget_seconds
            while True:
                if helper.poll() is not None or time.monotonic() >= startup_deadline:
                    raise Failure('TOKEN_HELPER_UNAVAILABLE')
                try:
                    with urlopen(base + '/ping', timeout=.3) as response:
                        ready = json.loads(response.read(4096))
                    if ready.get('version') != '2.0.2':
                        raise Failure('TOKEN_HELPER_VERSION_MISMATCH')
                    break
                except OSError:
                    time.sleep(.05)
            files, fingerprints = [], []
            for index, video in enumerate([args.left_video, args.right_video]):
                deadline = time.monotonic() + args.budget_seconds
                attempts = []
                source_slot = len(report['sources'])
                for selector in [FORMAT, 'bestaudio[ext=webm][protocol^=http][protocol!*=dash]']:
                    try:
                        command = [str(tools/'yt-dlp'), '--ignore-config', '--no-cache-dir', '--no-plugin-dirs', '--plugin-dirs', str(provider),
                                   '--no-remote-components', '--no-js-runtimes', '--js-runtimes', 'node:' + str(tools/'node'),
                                   '--extractor-args', f'youtubepot-bgutilhttp:base_url={base}',
                                   '--extractor-args', 'youtubepot-bgutilscript:server_home=' + str(scratch/'no-script'),
                                   '--extractor-args', 'youtube:player_client=mweb', '--dump-single-json', '--no-playlist', '--skip-download',
                                   '--format', FORMAT, '--', 'https://www.youtube.com/watch?v=' + video]
                        stage = 'metadata'
                        command[command.index('--format') + 1] = selector
                        info = json.loads(bounded(command, env, deadline))
                        if info.get('id') != video or not isinstance(info.get('url'), str):
                            raise Failure('EXTRACTED_ID_MISMATCH')
                        public = public_identity(info)
                        if len(report['sources']) == source_slot:
                            report['sources'].append(public)
                        else:
                            report['sources'][source_slot] = public
                        public['formatPolicy'] = 'production-progressive' if selector == FORMAT else 'same-video-alternate-webm-analysis'
                        public['formatAttempts'] = attempts
                        path = scratch / f'clip-{index}.wav'
                        decode = [str(tools/'ffmpeg'), '-nostdin', '-hide_banner', '-loglevel', 'error', '-rw_timeout', '10000000']
                        headers = ''.join(f'{key}: {value}\r\n' for key, value in info.get('http_headers', {}).items() if key.lower() in ['user-agent','referer','origin','accept'])
                        if headers:
                            decode += ['-headers', headers]
                        if args.position:
                            decode += ['-ss', str(args.position)]
                        decode += ['-i', info['url'], '-t', str(args.seconds), '-vn', '-ar', '11025', '-ac', '1', '-c:a', 'pcm_s16le', '-y', str(path)]
                        stage = 'pcm-decode'
                        bounded(decode, env, deadline)
                        with wave.open(str(path)) as stream:
                            public['capturedFrames'] = stream.getnframes()
                            public['capturedSeconds'] = stream.getnframes()/stream.getframerate()
                        if public['capturedSeconds'] < args.seconds-.1:
                            raise Failure('SHORT_PCM')
                        public['pcmSha256'] = hashlib.sha256(path.read_bytes()).hexdigest()
                        attempts.append(dict(formatId=info.get('format_id'), ok=True))
                        break
                    except Failure as error:
                        attempts.append(dict(formatId=locals().get('info', {}).get('format_id'), ok=False, failure=str(error), stage=stage))
                        if str(error) != 'STREAM_DENIED' or len(attempts) == 2:
                            raise
                stage = 'fingerprint'
                fingerprint = json.loads(bounded([str(tools/'fpcalc'), '-raw', '-json', '-length', str(args.seconds), str(path)], env, deadline))
                values = fingerprint.get('fingerprint')
                if isinstance(values, str):
                    values = [int(value) for value in values.split(',')]
                if not isinstance(values, list) or not values:
                    raise Failure('FINGERPRINT_UNAVAILABLE')
                public['fingerprint'] = values
                files.append(path); fingerprints.append(values)
                del info
            stage = 'alignment'
            report['chromaprintAlignment'] = fingerprint_alignment(*fingerprints)
            try:
                report['pcmAlignment'] = sample_alignment(*files)
            except ImportError:
                report['pcmAlignment'] = dict(available=False, failure='NUMPY_UNAVAILABLE')
            corr = report['pcmAlignment'].get('absoluteCorrelation', 0)
            agreement = (report['chromaprintAlignment'] or {}).get('bitAgreement', 0)
            if corr < .85 and agreement >= .9:
                report['rateAdjustedPcmAlignment'] = sample_alignment(*files, rate_search=True)
                corr = report['rateAdjustedPcmAlignment'].get('absoluteCorrelation', corr)
            report['excerptConclusion'] = 'strong_same_excerpt_signal_evidence' if corr >= .85 and agreement >= .8 else 'fingerprint_support_without_sample_alignment' if agreement >= .9 else 'equivalence_not_established'
            report['analysisCompleted'] = True
        except (Failure, OSError, ValueError, KeyError) as error:
            report['failures'].append(str(error) if isinstance(error, Failure) else 'ANALYSIS_FAILED')
            report['analysisCompleted'] = False
            report['failureStage'] = stage
        finally:
            if helper and helper.poll() is None:
                owned = terminate_owned_tree(helper.pid, signal.SIGTERM)
                try:
                    helper.wait(timeout=1)
                except subprocess.TimeoutExpired:
                    terminate_owned_tree(helper.pid, signal.SIGKILL, owned); helper.wait()
            report['helperStopped'] = helper is None or helper.poll() is not None
    report['sourceSha256'] = hashlib.sha256(Path(__file__).read_bytes()).hexdigest()
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(json.dumps(report, indent=2, allow_nan=False)+'\n')
    print(json.dumps({key:report.get(key) for key in ['analysisCompleted','excerptConclusion','failures','helperStopped']}))
    return 0 if report.get('analysisCompleted') else 1


if __name__ == '__main__':
    raise SystemExit(main())
