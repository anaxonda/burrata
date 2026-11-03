#!/usr/bin/env python3
"""Sample a private native playback process set using unprivileged /proc schedstat."""
from __future__ import annotations

import argparse
import json
import os
from pathlib import Path
import signal
import statistics
import sys
import time


THREAD_CLASSES = {
    'main', 'audio', 'io', 'worker', 'gdbus', 'webkit', 'pipewire',
    'wireplumber', 'recorder', 'other',
}


def parse_schedstat(raw: str | bytes) -> tuple[int, int, int] | None:
    try:
        if isinstance(raw, bytes):
            raw = raw.decode('ascii')
        fields = raw.split()
        if len(fields) != 3:
            return None
        values = tuple(int(field, 10) for field in fields)
        if any(value < 0 for value in values):
            return None
        return values
    except (UnicodeDecodeError, ValueError):
        return None


def schedstat_delta(before: tuple[int, int, int] | None,
                    after: tuple[int, int, int] | None) -> dict | None:
    if before is None or after is None:
        return None
    if any(new < old for old, new in zip(before, after)):
        return None
    running, runqueue, slices = (new - old for old, new in zip(before, after))
    return {'runningNs': running, 'runqueueWaitNs': runqueue, 'timeslices': slices}


def thread_class(name: str) -> str:
    lowered = name.lower()
    if lowered in {'burrata', 'burrata-main', 'main'}:
        return 'main'
    if 'wireplumber' in lowered:
        return 'wireplumber'
    if 'pipewire' in lowered or lowered.startswith('pw-'):
        return 'pipewire'
    if any(token in lowered for token in ('audio', 'cpal', 'rodio', 'pulse')):
        return 'audio'
    if any(token in lowered for token in ('io', 'reader', 'decoder')):
        return 'io'
    if any(token in lowered for token in ('webkit', 'webprocess')):
        return 'webkit'
    if any(token in lowered for token in ('gdbus', 'dbus')):
        return 'gdbus'
    if 'parec' in lowered:
        return 'recorder'
    if any(token in lowered for token in ('worker', 'pool', 'tokio', 'thread')):
        return 'worker'
    return 'other'


def read_cpu_pressure() -> dict:
    totals={'someTotalUs':None,'fullTotalUs':None}
    try:
        for line in Path('/proc/pressure/cpu').read_text(encoding='ascii').splitlines():
            fields=line.split()
            if not fields or fields[0] not in {'some','full'}:
                continue
            for field in fields[1:]:
                if field.startswith('total='):
                    value=field.partition('=')[2]
                    if value.isdecimal():totals[fields[0]+'TotalUs']=int(value)
    except OSError:
        pass
    return totals


def summarize_interval(samples: list[dict], start_ns: int, end_ns: int) -> dict:
    before = next((sample for sample in reversed(samples) if sample.get('monotonicNs', 0) <= start_ns), None)
    after = next((sample for sample in samples if sample.get('monotonicNs', 0) >= end_ns), None)
    if before is None or after is None or end_ns < start_ns:
        return {'available': False, 'durationNs': max(0, end_ns - start_ns), 'threads': []}
    def keyed(sample):
        return {(row['role'], row['processIndex'], row['threadIndex']): row for row in sample.get('tasks', [])}
    left, right = keyed(before), keyed(after)
    rows = []
    for key in sorted(set(left) | set(right)):
        old, new = left.get(key), right.get(key)
        if old is None or new is None:
            rows.append({'role': key[0], 'processIndex': key[1], 'threadIndex': key[2],
                         'threadClass': (new or old)['threadClass'], 'available': False,
                         'reason': 'task_missing_at_interval_boundary'})
            continue
        delta = schedstat_delta((old['runningNs'], old['runqueueWaitNs'], old['timeslices']),
                                (new['runningNs'], new['runqueueWaitNs'], new['timeslices']))
        rows.append({'role': key[0], 'processIndex': key[1], 'threadIndex': key[2],
                     'threadClass': new['threadClass'], 'available': delta is not None,
                     **(delta or {'reason': 'counter_reset_or_invalid'})})
    role_totals = {}
    for row in rows:
        if not row.get('available'):
            continue
        total = role_totals.setdefault(row['role'], {'threadCount': 0, 'runningNs': 0,
                                                     'runqueueWaitNs': 0, 'timeslices': 0})
        total['threadCount'] += 1
        for field in ('runningNs', 'runqueueWaitNs', 'timeslices'):
            total[field] += row[field]
    return {'available': True, 'durationNs': end_ns - start_ns,
            'sampleStartNs': before['monotonicNs'], 'sampleEndNs': after['monotonicNs'],
            'boundarySkewNs': {'start': start_ns - before['monotonicNs'],
                               'end': after['monotonicNs'] - end_ns},
            'roleTotals': role_totals, 'threads': rows}


def enrich_phase_intervals(result: dict, runtime: dict) -> None:
    marker_rows={row['phase']:row for row in result.get('markers', [])}
    markers = {key:row['monotonicNs'] for key,row in marker_rows.items()}
    play_ns = markers.get('play_request')
    observed_ns = markers.get('first_playing_snapshot_observed')
    if not isinstance(play_ns, int) or not isinstance(observed_ns, int):
        result['phaseIntervals'] = {'available': False, 'reason': 'playback_markers_missing'}
        return
    snap = (((runtime.get('startupDiagnosticExport') or {}).get('playingSnapshot')) or {})
    trace = snap.get('operationTrace') or []
    offsets = {row.get('phase'): row.get('atMs') for row in trace
               if row.get('phase') in {'source_ready', 'first_audio'}
               and isinstance(row.get('atMs'), int) and 0 <= row['atMs'] <= 60000}
    interval = summarize_interval(result.get('samples', []), play_ns, observed_ns)
    playing_marker = next((row for row in result.get('markers', [])
                           if row['phase'] == 'first_playing_snapshot_observed'), {})
    marker_metrics=playing_marker.get('metrics',{})
    marker_metrics={key:value for key,value in marker_metrics.items()
                    if key in {'generation','firstAudioMs','sourceReadyMs','operationFirstAudioMs'}
                    and isinstance(value,int) and 0<=value<=60000} if isinstance(marker_metrics,dict) else {}
    calibration=calibrate_wall_monotonic(list(marker_rows.values()))
    event_joins=[]
    diagnostic=(runtime.get('startupDiagnosticExport') or {})
    target_generation=diagnostic.get('targetGeneration')
    if calibration.get('withinBound') and isinstance(target_generation,int):
        decoder_events=[event for event in diagnostic.get('decoderEvents',[])
                        if event.get('generation')==target_generation
                        and isinstance(event.get('atUnixMs'),int)]
        native_events=[event for event in diagnostic.get('nativePlaybackEvents',[])
                       if event.get('generation')==target_generation
                       and event.get('status')=='playing' and isinstance(event.get('atUnixMs'),int)]
        first_playing=min(native_events,key=lambda event:event['atUnixMs']) if native_events else None
        starts=[event for event in decoder_events if event.get('phase')=='decoder_start']
        ends=[event for event in decoder_events if event.get('phase')=='decoder_end']
        if len(starts)==1 and len(ends)==1:
            event_joins.append(_aligned_event_interval(result.get('samples',[]),calibration,
                'decoder_start_to_end',starts[0],ends[0]))
            if first_playing:
                event_joins.append(_aligned_event_interval(result.get('samples',[]),calibration,
                    'decoder_end_to_first_playing_snapshot',ends[0],first_playing))
    result['phaseIntervals'] = {
        'scope': 'broad external interval from Node immediately before MCP play request until the first matching playing status response; includes transport and status-poll delay, not the exact internal output interval',
        'playRequestMarkerNs': play_ns,
        'firstPlayingSnapshotMarkerNs': observed_ns,
        'firstPlayingSnapshotMetrics': marker_metrics,
        'internalOperationTraceOffsetsMs': offsets,
        'internalOffsetsAlignedToNodeMarkers': False,
        'interval': interval,
        'wallMonotonicCalibration':calibration,
        'generationBoundEventJoins':event_joins,
    }


def calibrate_wall_monotonic(markers:list[dict],max_spread_ms:int=10)->dict:
    usable=[row for row in markers if isinstance(row.get('wallClockUnixMs'),int)
            and isinstance(row.get('monotonicNs'),int)
            and isinstance(row.get('clockPairSpanNs'),int)
            and row['clockPairSpanNs']>=0]
    if len(usable)<2:
        return {'available':False,'withinBound':False,'reason':'insufficient_clock_pairs','pairCount':len(usable)}
    offsets=sorted(row['monotonicNs']-row['wallClockUnixMs']*1_000_000 for row in usable)
    spread=offsets[-1]-offsets[0]
    pair_span=max(row['clockPairSpanNs'] for row in usable)
    uncertainty=spread+pair_span+2_000_000
    return {'available':True,'withinBound':uncertainty<=max_spread_ms*1_000_000,
            'pairCount':len(usable),'offsetMedianNs':offsets[len(offsets)//2],
            'offsetSpreadNs':spread,'maxClockPairSpanNs':pair_span,
            'uncertaintyBoundNs':uncertainty,'maxAllowedUncertaintyNs':max_spread_ms*1_000_000,
            'reason':None if uncertainty<=max_spread_ms*1_000_000 else 'clock_join_uncertainty_exceeds_bound'}


def _aligned_event_interval(samples:list[dict],calibration:dict,name:str,start:dict,end:dict)->dict:
    start_unix=start.get('atUnixMs');end_unix=end.get('atUnixMs')
    uncertainty=calibration['uncertaintyBoundNs']
    if not isinstance(start_unix,int) or not isinstance(end_unix,int) or end_unix<start_unix:
        return {'name':name,'available':False,'reason':'event_timestamp_invalid'}
    start_ns=start_unix*1_000_000+calibration['offsetMedianNs']
    end_ns=end_unix*1_000_000+calibration['offsetMedianNs']
    duration=end_ns-start_ns
    bounded_duration={'minimumNs':max(0,duration-2*uncertainty),
                      'maximumNs':duration+2*uncertainty}
    return {'name':name,'available':True,'timestampSemantics':'event-record timestamp, not hardware output time',
            'startEventPhase':start.get('phase',start.get('moment')),
            'endEventPhase':end.get('phase',end.get('moment')),
            'generation':start.get('generation'),'endGeneration':end.get('generation'),
            'startEstimateNs':start_ns,'endEstimateNs':end_ns,
            'durationEstimateNs':duration,'durationBoundNs':bounded_duration,
            'clockUncertaintyPerEventNs':uncertainty,
            'schedstatAtEstimatedEventBounds':summarize_interval(samples,start_ns,end_ns)}


def _task_rows(targets: list[dict], identities: dict[tuple[str, int], int],
               metadata: dict[tuple[str, int, int], dict], refresh: bool) -> tuple[list[dict], int]:
    rows = []
    failures = 0
    for target in targets:
        role = target.get('role')
        pid = target.get('pid')
        if role not in {'burrata', 'burrata-child', 'audio-controller', 'pipewire', 'pipewire-pulse', 'wireplumber', 'parec', 'xvfb'}:
            failures += 1
            continue
        if not isinstance(pid, int) or pid <= 0:
            failures += 1
            continue
        active=target.setdefault('_activeTids',{})
        if refresh or '_initialized' not in target:
            try:
                task_root = Path(f'/proc/{pid}/task')
                tids = sorted(int(path.name) for path in task_root.iterdir() if path.name.isdigit())
            except OSError:
                failures += 1
                continue
            active.clear()
            process_key = (role, pid)
            if process_key not in identities:
                identities[process_key] = len([key for key in identities if key[0] == role])
            process_index = identities[process_key]
            for tid in tids:
                key=(role,pid,tid)
                if key not in metadata:
                    try:
                        comm=Path(f'/proc/{pid}/task/{tid}/comm').read_text().strip()
                    except OSError:
                        failures+=1
                        continue
                    metadata[key]={'threadIndex':len(target.setdefault('_threadIndexByTid',{})),
                                   'threadClass':thread_class(comm)}
                    target['_threadIndexByTid'][tid]=metadata[key]['threadIndex']
                active[tid]=metadata[key]
            target['_initialized']=True
        process_key=(role,pid)
        if process_key not in identities:
            identities[process_key]=len([key for key in identities if key[0]==role])
        process_index=identities[process_key]
        for tid,thread_info in list(active.items()):
            try:
                stat=parse_schedstat(Path(f'/proc/{pid}/task/{tid}/schedstat').read_bytes())
            except OSError:
                failures+=1
                active.pop(tid,None)
                continue
            if stat is None:
                failures+=1
                active.pop(tid,None)
                continue
            rows.append({'role':role,'processIndex':process_index,
                         'threadIndex':thread_info['threadIndex'],'threadClass':thread_info['threadClass'],
                         'runningNs':stat[0],'runqueueWaitNs':stat[1],'timeslices':stat[2]})
    return rows, failures


def run_sampler(output: Path, marker_path: Path, interval_ms: int, targets: list[dict]) -> None:
    stop = False

    def request_stop(_signum, _frame):
        nonlocal stop
        stop = True

    signal.signal(signal.SIGTERM, request_stop)
    signal.signal(signal.SIGINT, request_stop)
    identities: dict[tuple[str, int], int] = {}
    metadata: dict[tuple[str, int, int], dict] = {}
    for target in targets:
        target['_activeTids'] = {}
        target['_threadIndexByTid'] = {}
    interval_ns = interval_ms * 1_000_000
    discover_interval_ns=100_000_000
    next_discovery=0
    samples = []
    failure_count = 0
    started = time.monotonic_ns()
    deadline = started
    while not stop:
        cycle_started = time.monotonic_ns()
        refresh=cycle_started>=next_discovery
        rows, failures = _task_rows(targets, identities, metadata, refresh)
        if refresh:next_discovery=cycle_started+discover_interval_ns
        pressure=read_cpu_pressure()
        sampled_at = time.monotonic_ns()
        failure_count += failures
        samples.append({'monotonicNs': sampled_at, 'readDurationNs': sampled_at - cycle_started,
                        'cpuPressure':pressure,'tasks': rows})
        deadline += interval_ns
        remaining = deadline - time.monotonic_ns()
        if remaining > 0:
            time.sleep(remaining / 1_000_000_000)
        elif -remaining > interval_ns * 20:
            # Rebase after a suspension instead of producing a busy catch-up loop.
            deadline = time.monotonic_ns()
    ended = time.monotonic_ns()
    markers = []
    try:
        for line in marker_path.read_text(encoding='utf-8').splitlines():
            marker = json.loads(line)
            if (marker.get('phase') in {'control_started', 'play_request', 'first_playing_snapshot_observed',
                                       'startup_observation_gate', 'pause_completed','diagnostics_report_received'}
                    and isinstance(marker.get('monotonicNs'), int) and marker['monotonicNs'] >= 0):
                row={'phase':marker['phase'],'monotonicNs':marker['monotonicNs']}
                wall=marker.get('wallClockUnixMs');span=marker.get('clockPairSpanNs')
                if isinstance(wall,int) and 1_000_000_000_000<=wall<=9_000_000_000_000:
                    row['wallClockUnixMs']=wall
                if isinstance(span,int) and 0<=span<=10_000_000:
                    row['clockPairSpanNs']=span
                metrics=marker.get('metrics')
                if marker['phase']=='first_playing_snapshot_observed' and isinstance(metrics,dict):
                    row['metrics']={key:value for key,value in metrics.items()
                        if key in {'generation','firstAudioMs','sourceReadyMs','operationFirstAudioMs'}
                        and isinstance(value,int) and 0<=value<=60000}
                markers.append(row)
    except (OSError, json.JSONDecodeError):
        pass
    read_durations=sorted(s['readDurationNs'] for s in samples)
    p95=read_durations[min(len(read_durations)-1, (len(read_durations)*95+99)//100-1)] if read_durations else 0
    intervals=sorted(samples[i]['monotonicNs']-samples[i-1]['monotonicNs'] for i in range(1,len(samples)))
    interval_p95=intervals[min(len(intervals)-1,(len(intervals)*95+99)//100-1)] if intervals else 0
    result = {'schemaVersion': 1, 'scope': 'private selected native playback/audio task schedstat plus process-global CPU pressure; diagnostic only',
              'clock': 'CLOCK_MONOTONIC nanoseconds', 'samplingIntervalMs': interval_ms,
              'threadDiscoveryIntervalMs':100,
              'startedMonotonicNs': started, 'endedMonotonicNs': ended,
              'actualElapsedNs': ended-started,'sampleCount': len(samples), 'failedReadCount': failure_count,
              'readDurationNs': {'median': int(statistics.median(s['readDurationNs'] for s in samples)) if samples else 0,
                                 'p95NearestRank':p95,'max': max((s['readDurationNs'] for s in samples), default=0)},
              'observedSampleIntervalNs':{'median':int(statistics.median(intervals)) if intervals else 0,
                                         'p95NearestRank':interval_p95,'max':max(intervals,default=0)},
              'cpuPressureScope':'system-wide PSI CPU pressure totals; not process-specific',
              'markers': markers, 'samples': samples}
    output.parent.mkdir(mode=0o700, parents=True, exist_ok=True)
    temporary = output.with_suffix(output.suffix + '.tmp')
    temporary.write_text(json.dumps(result, separators=(',', ':')) + '\n', encoding='utf-8')
    temporary.chmod(0o600)
    os.replace(temporary, output)


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument('--output', type=Path, required=True)
    parser.add_argument('--markers', type=Path, required=True)
    parser.add_argument('--interval-ms', type=int, choices=(10,), default=10)
    args = parser.parse_args()
    if not str(args.output).startswith('/tmp/burrata-launch.') or not str(args.markers).startswith('/tmp/burrata-launch.'):
        parser.error('sampler paths must be inside the private temporary profile')
    targets = json.load(sys.stdin)
    if not isinstance(targets, list) or not targets:
        parser.error('a non-empty private target list is required on stdin')
    run_sampler(args.output, args.markers, args.interval_ms, targets)
    return 0


if __name__ == '__main__':
    raise SystemExit(main())
