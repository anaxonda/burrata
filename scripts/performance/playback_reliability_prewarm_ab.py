#!/usr/bin/env python3
"""Bounded alternating foreground-playback helper prewarm experiment.

Exactly three matched pairs by default. This is exploratory, not p95/recording
qualification. Each example invocation isolates its Store and child XDG roots.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import signal
import statistics
import subprocess

from playback_reliability import terminate_owned_tree


def stages(path):
    events = []
    for line in path.read_text().splitlines():
        if line.startswith('burrata_playback_phase '):
            events.append(json.loads(line.removeprefix('burrata_playback_phase ')))
    def first(phase):
        return next((event['elapsedMs'] for event in events if event['phase'] == phase), None)
    def span(start, end):
        a, b = first(start), first(end)
        return b-a if a is not None and b is not None else None
    ready = next((event['details'].get('durationMs') for event in events if event['phase'] == 'provider_helper_ready_end'), None)
    probe = [event['elapsedMs'] for event in events if event['phase'] in ['stream_probe_start','stream_probe_end','stream_probe_body_end']]
    return dict(searchMs=span('search_start','search_end'), extractionMs=span('extraction_start','extraction_end'),
                helperReadyWaitMs=ready, streamProbeSpanMs=max(probe)-min(probe) if probe else None,
                probeDeniedResponses=sum(event.get('details',{}).get('status')==403 for event in events if event['phase']=='stream_probe_end'))


def compare(rows):
    pairs = []
    for number in sorted({row['pair'] for row in rows}):
        group = {row['mode']:row for row in rows if row['pair']==number}
        before, after = group.get('baseline',{}), group.get('prewarm',{})
        same = bool(before.get('identity') and before.get('identity')==after.get('identity'))
        matched = same and before.get('ok') and after.get('ok')
        result = dict(pair=number, sameProviderVideoId=same, matched=bool(matched))
        if matched:
            for key in ['firstPcmFromRequestMs','resolveMs']:
                if all(isinstance(row.get(key),(int,float)) for row in [before,after]):
                    result[key+'Saved'] = before[key]-after[key]
        pairs.append(result)
    improvements = [pair['firstPcmFromRequestMsSaved'] for pair in pairs if 'firstPcmFromRequestMsSaved' in pair]
    return dict(pairs=pairs, matchedPairs=len(improvements),
                medianFirstPcmMsSaved=statistics.median(improvements) if improvements else None,
                meanFirstPcmMsSaved=statistics.mean(improvements) if improvements else None,
                p95=None, p95UnavailableReason='exploratory three-per-path sample; no 20-repetition qualification')


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary',type=lambda value:Path(value).resolve(),required=True)
    parser.add_argument('--track',type=lambda value:Path(value).resolve(),required=True)
    parser.add_argument('--settings',type=lambda value:Path(value).resolve(),required=True)
    parser.add_argument('--output-directory',type=Path,required=True)
    parser.add_argument('--pairs',type=int,default=3)
    args=parser.parse_args()
    if not 1<=args.pairs<=3:
        parser.error('initial bounded experiment allows one to three pairs')
    settings=json.loads(args.settings.read_text())
    timeout=settings.get('playbackStartupTimeoutSeconds',20)
    if timeout not in [20,30,45]:parser.error('invalid shared startup deadline')
    directory=args.output_directory
    directory.mkdir(parents=True,exist_ok=True)
    report=dict(schema='burrata.provider-prewarm-ab/v1',qualification=False,exploratory=True,warmups=0,
                pairs=args.pairs,binarySha256=hashlib.sha256(args.binary.read_bytes()).hexdigest(),
                scope='production catalog resolver plus five-second FFmpeg PCM; foreground helper prewarm only; no acoustic or device qualification',
                limitations=['No p95 or release performance claim from three observations per path.',
                             'Cold means new Store/acquisition context and stopped owned provider; OS cache is retained.',
                             'Different selected provider/video IDs or failures are excluded from matched latency deltas but retained.'],attempts=[])
    for pair in range(1,args.pairs+1):
        for mode in (['baseline','prewarm'] if pair%2 else ['prewarm','baseline']):
            output=directory/f'pair-{pair:02}-{mode}.json'
            log=directory/f'pair-{pair:02}-{mode}.log'
            command=[str(args.binary),'--track',str(args.track),'--settings',str(args.settings),'--output',str(output),
                     '--scenario','cold','--smoke','--repetitions','1','--warmups','0']
            if mode=='prewarm':command.append('--prewarm-provider')
            with log.open('wb') as errors:
                child=subprocess.Popen(command,stdout=subprocess.DEVNULL,stderr=errors,stdin=subprocess.DEVNULL,
                                       env=dict(os.environ,BURRATA_PLAYBACK_TRACE='1'),start_new_session=True)
                timed_out=False
                try:child.wait(timeout=timeout+8)
                except subprocess.TimeoutExpired:
                    timed_out=True; owned=terminate_owned_tree(child.pid,signal.SIGTERM)
                    try:child.wait(timeout=1)
                    except subprocess.TimeoutExpired:terminate_owned_tree(child.pid,signal.SIGKILL,owned);child.wait()
            row=dict(pair=pair,mode=mode,exitCode=child.returncode,ok=False)
            if timed_out:row['failure']='HARNESS_TIMEOUT'
            elif output.exists():
                raw=json.loads(output.read_text());attempt=(raw.get('attempts') or [{}])[0]
                identity=attempt.get('resolvedRecordingIdentity') or {}
                row.update(ok=bool(attempt.get('ok')),identity=dict(provider=identity.get('provider'),providerId=identity.get('providerId')),
                           firstPcmFromRequestMs=attempt.get('firstPcmFromRequestMs'),resolveMs=attempt.get('resolveMs'),
                           failure=attempt.get('failure'),providerPrewarm=attempt.get('providerPrewarm'),
                           providerShutdownOk=raw.get('providerShutdownOk'),stageMetrics=stages(log))
                if raw.get('providerPrewarmEnabled')!=(mode=='prewarm'):
                    row.update(ok=False,failure='PREWARM_PROTOCOL_MISMATCH')
            else:row['failure']='MISSING_REPORT'
            report['attempts'].append(row);report['comparison']=compare(report['attempts'])
            (directory/'comparison.json').write_text(json.dumps(report,indent=2,allow_nan=False)+'\n')
            print(json.dumps(row),flush=True)
    report['runCompleted']=True
    (directory/'comparison.json').write_text(json.dumps(report,indent=2,allow_nan=False)+'\n')
    return 0 if all(row['ok'] for row in report['attempts']) else 1


if __name__=='__main__':raise SystemExit(main())
