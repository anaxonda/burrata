#!/usr/bin/env python3
"""Opt-in, bounded fresh-process VISIONOS diagnostics; importing makes no requests."""
from __future__ import annotations
import argparse
import json
import math
import os
import re
import subprocess
import tempfile
import time
from pathlib import Path

from serial_same_worker import BoundedLineReader, ProtocolError, terminate_group, validate_tracks, file_sha256

ROOT = Path(__file__).resolve().parents[3]
HERE = Path(__file__).resolve().parent
NODE = ROOT / 'src-tauri/resources/tools/x86_64-unknown-linux-gnu/node'
FFMPEG = NODE.with_name('ffmpeg')
WORKER = HERE / 'visionos_probe.mjs'
MANIFEST = HERE / 'fixtures/transport-music-tracks.json'
CODEC = 'audio/webm; codecs="opus"'
V2 = 'innertubex-93b-profile-meld-9c3-defaults-v2'
V3 = 'innertubex-93b-profile-meld-9c3-anonymous-visitor-v3'
SOURCE_FILES = [Path(__file__), HERE / 'serial_same_worker.py', WORKER, HERE / 'core.mjs',
                HERE / 'budget.mjs', HERE / 'measure_child.py', HERE / 'fixtures/visionos-01-source-request.json',
                HERE / 'fixtures/visionos-01-anonymous-visitor-source.json', NODE, FFMPEG]
STAGES = {'visitorServiceWorker', 'visitorHomepage', 'player', 'parse', 'format', 'decode', 'request'}
NETWORK_CAUSES = {'ECONNRESET', 'UND_ERR_SOCKET', 'ETIMEDOUT', 'ENOTFOUND', 'EAI_AGAIN', 'OTHER'}
HEADERS = {'Accept', 'Accept-Language', 'Cache-Control', 'Content-Type', 'Origin', 'Referer', 'User-Agent',
           'X-Goog-Api-Format-Version', 'X-Goog-Visitor-Id', 'X-YouTube-Client-Name', 'X-YouTube-Client-Version', 'X-Origin'}
FAILURES = set('INVALID_PLAYBACK_REQUEST INVALID_BUDGET INVALID_JSON TIMEOUT CANCELLED NETWORK_FAILED '
    'PLAYER_HTTP_ERROR RESPONSE_TOO_LARGE INVALID_RESPONSE PLAYABILITY_REJECTED RECORDING_MISMATCH '
    'METADATA_MISSING METADATA_MISMATCH FORMAT_UNAVAILABLE FORMAT_MISMATCH CIPHER_REQUIRED N_TRANSFORM_REQUIRED '
    'UNSUPPORTED_TRANSPORT INVALID_MEDIA_URL FORMAT_LENGTH_MISSING FORMAT_DURATION_MISSING PREVIEW_OR_TRUNCATED_FORMAT '
    'MANIFEST_EXPIRED DECODE_FAILED DECODER_UNAVAILABLE STREAM_DENIED NO_PCM SHORT_AUDIO VISITOR_CONTEXT_UNAVAILABLE '
    'INVALID_DECODE_RANGE FULL_RECORDING_TOO_LONG TRUNCATED_RECORDING FULL_RECORDING_DURATION_MISMATCH'.split())
NUMBERS = set('httpStatus requestedFormatId formatId contentLength approximateDurationMs expiresInMs firstPcmMs '
    'firstPcmFromCommandMs decodedFrames decodedAudioMs decodeWallMs decoderCpuMs decoderMaxRssBytes '
    'accountingWrapperCpuMs accountingWrapperMaxRssBytes helperCpuMs helperRssBytes helperMaxRssBytes '
    'totalCpuMs wallMs positionSeconds expectedDecodeSeconds decoderLimitSeconds minimumDecodedFrames'.split())
BOOLS = {'ok', 'qualification', 'transportParity', 'identityMatchesRequestedId', 'fullRecording',
         'fullRecordingComplete', 'accountingComplete'}


def number(value):
    return isinstance(value, (int, float)) and not isinstance(value, bool) and math.isfinite(value) and value >= 0


def safe_response(raw, expected_id):
    try:
        value = json.loads(raw)
    except (UnicodeError, ValueError) as error:
        raise ProtocolError('invalid response JSON') from error
    if not isinstance(value, dict) or value.get('id') != expected_id:
        raise ProtocolError('response identity mismatch')
    safe = {'id': expected_id}
    for key in NUMBERS:
        if number(value.get(key)):
            safe[key] = value[key]
    for key in BOOLS:
        if isinstance(value.get(key), bool):
            safe[key] = value[key]
    allowed_strings = {'profileId': {'VISIONOS_0_1'}, 'constructionVersion': {V2, V3},
        'tokenWork': {'not-requested'}, 'playerJavaScript': {'not-requested'}, 'codec': {CODEC},
        'failure': FAILURES, 'stage': STAGES,
        'networkCause': NETWORK_CAUSES,
        'playabilityStatus': {'OK', 'UNPLAYABLE', 'LOGIN_REQUIRED', 'ERROR', 'LIVE_STREAM_OFFLINE', 'UNKNOWN'},
        'playabilityCategory': {'OK', 'AGE_RESTRICTED', 'LOGIN_REQUIRED', 'UNAVAILABLE', 'UNCLASSIFIED_REJECTION'},
        'visitorContext': {'omitted-nullable-profile; no-session-fetch', 'anonymous-source-pending',
            'anonymous-source-service-worker; ephemeral-no-session-cache',
            'anonymous-source-music-homepage; ephemeral-no-session-cache'}}
    for key, allowed in allowed_strings.items():
        if isinstance(value.get(key), str) and value[key] in allowed:
            safe[key] = value[key]
    for key in ('requestedVideoId', 'returnedVideoId'):
        if isinstance(value.get(key), str) and re.fullmatch(r'[A-Za-z0-9_-]{11}', value[key]):
            safe[key] = value[key]
    for key in ('requestHeaderNames', 'mediaHeaderNames'):
        headers = value.get(key)
        if isinstance(headers, list) and all(isinstance(item, str) and item in HEADERS for item in headers):
            safe[key] = headers
    metadata = value.get('metadata')
    if isinstance(metadata, dict):
        safe['metadata'] = {key: metadata[key] for key in ('titlePresent', 'authorPresent', 'matchesExpectedMetadata')
                            if isinstance(metadata.get(key), bool)}
        if number(metadata.get('durationSeconds')):
            safe['metadata']['durationSeconds'] = metadata['durationSeconds']
        if isinstance(metadata.get('videoId'), str) and re.fullmatch(r'[A-Za-z0-9_-]{11}', metadata['videoId']):
            safe['metadata']['videoId'] = metadata['videoId']
    if isinstance(value.get('stages'), dict):
        safe['stages'] = {key: item for key, item in value['stages'].items() if key in STAGES and number(item)}
    attempts = value.get('visitorAttempts')
    if isinstance(attempts, list) and len(attempts) <= 2:
        safe['visitorAttempts'] = []
        for attempt in attempts:
            if not isinstance(attempt, dict) or attempt.get('source') not in {'service-worker', 'music-homepage'}:
                continue
            item = {'source': attempt['source']}
            for key in ('found', 'httpStatus'):
                if (key == 'found' and isinstance(attempt.get(key), bool)) or (key == 'httpStatus' and number(attempt.get(key))):
                    item[key] = attempt[key]
            if isinstance(attempt.get('networkCause'), str) and attempt['networkCause'] in NETWORK_CAUSES:
                item['networkCause'] = attempt['networkCause']
            safe['visitorAttempts'].append(item)
    return safe


def plan_requests(tracks, mode, attempts=20):
    validate_tracks(tracks)
    if any(not re.fullmatch(r'[A-Za-z0-9_-]{1,40}', row['name']) for row in tracks):
        raise ValueError('unsafe track label')
    if mode not in {'exploratory', 'seek', 'full', 'durability', 'cold'} or not 1 <= attempts <= 60:
        raise ValueError('invalid diagnostic plan')
    if mode in {'full', 'durability'} and any(row['durationSeconds'] > 600 for row in tracks):
        raise ValueError('full recording exceeds diagnostic duration cap')
    plan = []
    if mode == 'cold':
        # A,B,C / B,C,A / C,A,B blocks rotate the first position as well as the IDs.
        for index in range(attempts):
            block, slot = divmod(index, 3)
            plan.append((tracks[(block + slot) % 3], 'cold', None, block + 1))
    else:
        for track in tracks:
            if mode == 'exploratory':
                plan.append((track, 'excerpt', None, 1))
            if mode in {'seek', 'durability'}:
                plan.extend((track, f'seek-{percent}', percent / 100, 1) for percent in (1, 50, 95))
            if mode in {'full', 'durability'}:
                plan.append((track, 'full', None, 1))
    return plan


def hashes(manifest):
    result = {}
    for path in [*SOURCE_FILES, manifest]:
        result[str(path.relative_to(ROOT)) if path.is_relative_to(ROOT) else path.name] = file_sha256(path)
    return result


def eligible(row, track, phase, fraction, visitor_context):
    metadata = row.get('metadata', {})
    duration = metadata.get('durationSeconds')
    frames = row.get('decodedFrames')
    full = phase == 'full'
    expected_position = duration * fraction if number(duration) and fraction is not None else 0
    return (row.get('ok') is True and row.get('qualification') is False and row.get('transportParity') is False and
        row.get('profileId') == 'VISIONOS_0_1' and row.get('constructionVersion') == (V3 if visitor_context else V2) and
        row.get('tokenWork') == 'not-requested' and row.get('playerJavaScript') == 'not-requested' and
        row.get('formatId') == 251 and row.get('codec') == CODEC and row.get('playabilityStatus') == 'OK' and
        row.get('requestedVideoId') == track['videoId'] and row.get('returnedVideoId') == track['videoId'] and
        row.get('identityMatchesRequestedId') is True and metadata.get('videoId') == track['videoId'] and
        metadata.get('matchesExpectedMetadata') is True and number(duration) and
        abs(duration - track['durationSeconds']) <= 1 and number(row.get('positionSeconds')) and
        abs(row['positionSeconds'] - expected_position) < 0.001 and row.get('fullRecording') is full and
        number(frames) and frames > 0 and (abs(frames / 48000 - duration) <= 1 if full else abs(frames / 48000 - 5) <= 0.1) and
        (not full or row.get('fullRecordingComplete') is True) and
        row.get('expectedDecodeSeconds') == (duration if full else 5) and
        row.get('decoderLimitSeconds') == (duration + 1 if full else 5) and
        number(row.get('decodedAudioMs')) and abs(row['decodedAudioMs'] - frames / 48) < 0.01 and
        row.get('mediaHeaderNames') == [] and row.get('accountingComplete') is True and
        all(number(row.get(key)) for key in ('decoderCpuMs', 'decoderMaxRssBytes', 'accountingWrapperCpuMs',
            'accountingWrapperMaxRssBytes', 'helperCpuMs', 'helperRssBytes', 'helperMaxRssBytes', 'totalCpuMs')) and
        number(row.get('firstPcmFromCommandMs')) and row['firstPcmFromCommandMs'] <= 19000 and
        (visitor_context or row.get('visitorContext') == 'omitted-nullable-profile; no-session-fetch') and
        (not visitor_context or (row.get('visitorContext') in {
            'anonymous-source-service-worker; ephemeral-no-session-cache', 'anonymous-source-music-homepage; ephemeral-no-session-cache'}
            and 'X-Goog-Visitor-Id' in row.get('requestHeaderNames', []))))


def run(output, *, mode, manifest=MANIFEST, attempts=20, visitor_context=False, outer_s=None, request_s=20, only=None):
    output = Path(output)
    if output.exists():
        raise ValueError('output already exists')
    manifest = Path(manifest)
    tracks = json.loads(manifest.read_text())
    plan = plan_requests(tracks, mode, attempts)
    if only is not None:
        if mode == 'cold':
            raise ValueError('single diagnostic selection is not a cold sample')
        plan = [item for item in plan if f'{item[0]["name"]}:{item[1]}' == only]
        if len(plan) != 1:
            raise ValueError('single diagnostic selection must match exactly one planned call')
    outer_s = min(600, len(plan) * 21 + 5) if outer_s is None else outer_s
    if not number(outer_s) or not 0 < outer_s <= 600 or not number(request_s) or not 0 < request_s <= 20:
        raise ValueError('invalid deadline')
    if not all(path.is_file() for path in (NODE, FFMPEG, WORKER)) or not all(os.access(path, os.X_OK) for path in (NODE, FFMPEG)):
        raise ValueError('missing local assets')
    initial_hashes = hashes(manifest)
    output.parent.mkdir(parents=True, exist_ok=True)
    with output.open('x') as reserved:
        reserved.write('{}\n')
    started = time.monotonic(); outer_deadline = started + outer_s
    rows = []

    def checkpoint(status):
        report = {'schema': 'visionos-fresh-process-diagnostics-v1', 'qualification': False,
            'status': status, 'mode': mode, 'visitorContextEnabled': visitor_context,
            'explicitSingleDiagnostic': only,
            'scope': 'three explicit public music uploads; duration-counted decoder PCM; no acoustic/catalog/device or production qualification',
            'timingOrigin': 'firstPcmFromCommandMs starts on request handling; process launch excluded; launchToResponseMs ends after decode',
            'startupPolicy': 'fresh Node and task XDG/TMP directories per call; HOME unchanged; no accounts or persistent visitor/URL cache',
            'formatId': 251, 'codec': CODEC, 'plannedRequests': len(plan), 'perRequestBudgetMs': 19000,
            'processDeadlineMs': request_s * 1000, 'outerBudgetMs': outer_s * 1000,
            'sourceSha256': initial_hashes, 'requests': rows,
            'eligibleRequests': sum(row.get('eligible') is True for row in rows),
            'totalWallMs': round((time.monotonic() - started) * 1000, 3)}
        with tempfile.NamedTemporaryFile(mode='w', dir=output.parent, prefix=output.name + '.', suffix='.tmp', delete=False) as temporary:
            temporary.write(json.dumps(report, indent=2) + '\n')
        Path(temporary.name).replace(output)
        return report

    checkpoint('running')
    try:
        for index, (track, phase, fraction, block) in enumerate(plan, 1):
            request_id = f'visionos-{index}-{track["name"]}-{phase}'
            row = {'id': request_id, 'track': track['name'], 'phase': phase, 'block': block, 'eligible': False}
            if time.monotonic() >= outer_deadline:
                rows.append({**row, 'status': 'not_run', 'failure': 'OUTER_DEADLINE'}); checkpoint('running'); continue
            with tempfile.TemporaryDirectory(prefix='burrata-visionos-driver-') as scratch:
                env = os.environ.copy()
                for key in tuple(env):
                    if key.startswith('BURRATA_BENCH_'):
                        env.pop(key)
                env['BURRATA_BENCH_FFMPEG'] = str(FFMPEG)
                for key, leaf in [('XDG_CONFIG_HOME', 'config'), ('XDG_CACHE_HOME', 'cache'), ('XDG_DATA_HOME', 'data'), ('TMPDIR', 'tmp')]:
                    path = Path(scratch) / leaf; path.mkdir(mode=0o700); env[key] = str(path)
                process = None; attempt_started = time.monotonic()
                deadline = min(attempt_started + request_s, outer_deadline)
                request = {'id': request_id, 'videoId': track['videoId'], 'expectedVideoId': track['videoId'],
                    'formatId': 251, 'hl': 'en', 'gl': 'US', 'budgetMs': max(1, min(19000, int((deadline - time.monotonic()) * 1000) - 500)),
                    'expectedMetadata': {key: track[key] for key in ('titleNeedle', 'channelNeedle', 'durationSeconds')}}
                if visitor_context:
                    request['visitorMode'] = 'anonymous-source'
                if phase == 'full':
                    request['fullRecording'] = True
                else:
                    request['seconds'] = 5
                if fraction is not None:
                    request['positionFraction'] = fraction
                try:
                    process = subprocess.Popen([str(NODE), str(WORKER)], cwd=ROOT, env=env,
                        stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.DEVNULL, bufsize=0, start_new_session=True)
                    process.stdin.write((json.dumps(request) + '\n').encode()); process.stdin.close()
                    row.update(safe_response(BoundedLineReader(process.stdout.fileno()).read(deadline, max_bytes=65536), request_id))
                    row['eligible'] = eligible(row, track, phase, fraction, visitor_context)
                    row['status'] = 'completed'
                except (TimeoutError, EOFError, ProtocolError, OSError, subprocess.SubprocessError) as error:
                    row.update(status='driver_failed', failure={TimeoutError: 'DRIVER_DEADLINE', EOFError: 'WORKER_EOF',
                        ProtocolError: 'PROTOCOL_ERROR'}.get(type(error), 'PROCESS_ERROR'))
                finally:
                    row['launchToResponseMs'] = round((time.monotonic() - attempt_started) * 1000, 3)
                    if process is not None:
                        terminate_group(process)
                        if not process.stdin.closed:
                            process.stdin.close()
                        process.stdout.close()
                    row['cleanupCompleted'] = process is None or process.poll() is not None
                    row['launchToCleanupMs'] = round((time.monotonic() - attempt_started) * 1000, 3)
                rows.append(row); checkpoint('running')
    except BaseException:
        checkpoint('interrupted')
        raise
    report = checkpoint('completed')
    if hashes(manifest) != initial_hashes:
        for row in rows:
            row['eligible'] = False
        report = checkpoint('source_changed_during_run')
    return report


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--output', type=Path, required=True)
    parser.add_argument('--mode', choices=['exploratory', 'seek', 'full', 'durability', 'cold'], required=True)
    parser.add_argument('--manifest', type=Path, default=MANIFEST)
    parser.add_argument('--attempts', type=int, default=20, help='cold calls in rotating three-ID blocks; max60')
    parser.add_argument('--visitor-context', action='store_true', help='explicit anonymous source visitor lifecycle (v3)')
    parser.add_argument('--outer-seconds', type=float)
    parser.add_argument('--only', help='one explicit track:phase from the chosen plan; never an automatic retry')
    args = parser.parse_args()
    result = run(args.output, mode=args.mode, manifest=args.manifest, attempts=args.attempts,
                 visitor_context=args.visitor_context, outer_s=args.outer_seconds, only=args.only)
    print(json.dumps({'status': result['status'], 'requests': len(result['requests']), 'eligible': result['eligibleRequests'], 'qualification': False}))
