#!/usr/bin/env python3
"""Opt-in fresh-process captured-source matrix. No GUI, account, catalog or device output."""
import argparse, hashlib, json, math, os, signal, statistics, subprocess, time
from pathlib import Path

TEST = 'playback_sources::direct_youtube_integration_tests::live_direct_youtube_foreground_source_fixture_pcm'
EXPERIMENT = 'production-direct-youtube-foreground-v1'

def sha(path):
    return hashlib.sha256(Path(path).read_bytes()).hexdigest()

def stats(values):
    values = sorted(values)
    return None if not values else dict(count=len(values), median=statistics.median(values),
        p95NearestRank=values[math.ceil(.95*len(values))-1], worst=max(values))

def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--run-isolated', action='store_true')
    parser.add_argument('--fixture-index', type=int, required=True)
    parser.add_argument('--binary', type=Path, required=True)
    parser.add_argument('--output', type=Path, required=True)
    args = parser.parse_args()
    if not args.run_isolated:
        print('Plan only: 3 exploratory +20 measured fresh processes, one fixed public ID; 120s action bound.')
        return
    binary = args.binary.resolve(strict=True)
    if args.output.exists():
        raise SystemExit('Refusing to overwrite an existing capture')
    if not Path('/usr/bin/time').is_file():
        raise SystemExit('GNU time is required for this Linux-only resource capture')
    args.output.mkdir(parents=True)
    signal.signal(signal.SIGTERM, lambda *_: (_ for _ in ()).throw(SystemExit('Interrupted')))
    discovery_path=Path('scripts/performance/innertube/results/2026-10-19-transport-track-discovery.json')
    discovery=json.loads(discovery_path.read_text())
    fixture=discovery['queries'][args.fixture_index]['results'][0]
    report = dict(sourceFixturePath=str(discovery_path),sourceFixtureSha256=sha(discovery_path),sourceFixtureIndex=args.fixture_index,schema='burrata.native-direct-foreground-matrix/v1', qualification=False,
        sourceCommit=subprocess.check_output(['git','rev-parse','HEAD'], text=True).strip(),
        binarySha256=sha(binary), driverSha256=sha(__file__), videoId=fixture['videoId'],
        scope='Exact public YouTube ID through native source resolution and five seconds of decoder PCM; no mixer/device/catalog/UI.',
        processPolicy='Fresh test process and anonymous HTTP client per trial; OS DNS/TLS/server state is not reset. No retry within a trial.',
        resourceScope='GNU time CPU includes waited descendants; maxRSS is maximum reported process RSS, not simultaneous summed tree memory or PSS. CPU is aggregate, not a peak.',
        limits=dict(exploratory=3, measured=20, perTrialSeconds=30, actionSeconds=120), rows=[])
    deadline = time.monotonic()+120
    env = dict(os.environ, BURRATA_DIRECT_YOUTUBE_LIVE='1', BURRATA_DIRECT_YOUTUBE_EXPERIMENT='1', BURRATA_PLAYBACK_TRACE='1',BURRATA_DIRECT_YOUTUBE_SOURCE_FIXTURE=json.dumps(fixture,ensure_ascii=False))
    for index in range(23):
        row = dict(index=index, exploratory=index<3, started=False, ok=False)
        report['rows'].append(row)
        if time.monotonic() >= deadline:
            row['failure'] = 'ACTION_BUDGET_NOT_STARTED'
            continue
        row['started'] = True
        log = args.output/f'trial-{index:02}.log'
        timing = args.output/f'trial-{index:02}.time.json'
        command = ['/usr/bin/time','-q','-f','{"userSeconds":%U,"systemSeconds":%S,"maxRssKiB":%M,"wallSeconds":%e,"exitCode":%x}',
            '-o',str(timing),str(binary),'--exact',TEST,'--ignored','--nocapture','--test-threads=1']
        with log.open('wb') as output:
            process = subprocess.Popen(command, env=env, stdout=output, stderr=subprocess.STDOUT, start_new_session=True)
            try:
                row['exitCode'] = process.wait(timeout=min(30,max(.01,deadline-time.monotonic())))
            except subprocess.TimeoutExpired:
                os.killpg(process.pid, signal.SIGKILL)
                process.wait()
                row.update(exitCode=process.returncode, failure='OWNED_GROUP_TIMEOUT')
            finally:
                if process.poll() is None:
                    os.killpg(process.pid, signal.SIGKILL)
                    process.wait()
        candidates=[]
        phases=[]
        for line in log.read_text(errors='replace').splitlines():
            if line.startswith('burrata_playback_phase '):
                try: phases.append(json.loads(line.split(' ',1)[1])['phase'])
                except (ValueError,KeyError): pass
            if line.startswith('{'):
                try:
                    value=json.loads(line)
                    if value.get('experiment')==EXPERIMENT: candidates.append(value)
                except ValueError: pass
        if len(candidates)==1:
            row['result']=candidates[0]
            row['ok']=row['exitCode']==0 and candidates[0].get('ok') is True
        else:
            row.setdefault('failure','MISSING_OR_DUPLICATE_RESULT')
        row['helperPreparationEvents']=phases.count('provider_prepare_start')
        row['extractorEvents']=phases.count('extraction_start')
        row['logSha256']=sha(log)
        if timing.exists():
            try: row['resources']=json.loads(timing.read_text())
            except ValueError: row['resourceFailure']='INCOMPLETE_GNU_TIME'
        (args.output/'report.json').write_text(json.dumps(report,indent=2)+'\n')
        print(json.dumps(dict(index=index, exploratory=row['exploratory'], ok=row['ok'], firstPcmMs=row.get('result',{}).get('firstPcmFromStartMs'))),flush=True)
    measured=[r for r in report['rows'] if not r['exploratory']]
    success=[r for r in measured if r['ok']]
    report['summary']=dict(measuredPlanned=20,measuredStarted=sum(r['started'] for r in measured),
        measuredPassed=len(success), measuredFailed=sum(r['started'] and not r['ok'] for r in measured),
        firstPcmFromStartMs=stats([r['result']['firstPcmFromStartMs'] for r in success]),
        resolutionMs=stats([r['result']['resolveMs'] for r in success]),
        processCpuSeconds=stats([r['resources']['userSeconds']+r['resources']['systemSeconds'] for r in success if 'resources' in r]),
        maxProcessRssKiB=stats([r['resources']['maxRssKiB'] for r in success if 'resources' in r]),
        helperPreparationEvents=sum(r.get('helperPreparationEvents',0) for r in report['rows']),
        extractorEvents=sum(r.get('extractorEvents',0) for r in report['rows']))
    (args.output/'report.json').write_text(json.dumps(report,indent=2)+'\n')
    print(json.dumps(report['summary'],indent=2),flush=True)

if __name__=='__main__':
    main()
