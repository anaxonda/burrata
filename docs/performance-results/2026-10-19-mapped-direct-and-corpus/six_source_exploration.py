import json,os,subprocess,time,hashlib,signal
from pathlib import Path
out=Path('/tmp/burrata-direct-six-source-e06');out.mkdir(exist_ok=False)
binary=Path('.cache/verification-target/debug/deps/burrata_lib-f37d34080c2fadac').resolve()
paths=['scripts/performance/innertube/results/2026-10-19-transport-track-discovery.json','scripts/performance/innertube/results/2026-10-19-matthew-excerpt-equivalence-final.json','docs/performance-results/2026-10-18-todo-release/youtube-public-import.json']
fixtures=[q['results'][0] for q in json.loads(Path(paths[0]).read_text())['queries'][:4]]
fixtures += [json.loads(Path(paths[1]).read_text())['sources'][0],json.loads(Path(paths[2]).read_text())['video']]
sha=lambda p:hashlib.sha256(Path(p).read_bytes()).hexdigest()
report={'schema':'burrata.direct-six-source-exploration/v1','sourceCommit':'e06bf2a','binarySha256':sha(binary),'driverSha256':sha(__file__),'qualification':False,'scope':'Explicit captured source IDs; five seconds decoder PCM, no catalog equivalence, mixer or physical output; fresh process per call, OS/network not reset. Three exploratory repetitions per ID, no retries.','sourceFixtures':{p:sha(p) for p in paths},'rows':[]}
deadline=time.monotonic()+120
for repetition in range(3):
 for index,fixture in enumerate(fixtures):
  row={'repetition':repetition,'fixtureIndex':index,'videoId':fixture['videoId'],'started':False,'ok':False};report['rows'].append(row)
  if time.monotonic()>=deadline:row['failure']='ACTION_BUDGET';continue
  row['started']=True
  log=out/f'{repetition}-{index}.log'
  env=dict(os.environ,BURRATA_DIRECT_YOUTUBE_LIVE='1',BURRATA_DIRECT_YOUTUBE_EXPERIMENT='1',BURRATA_PLAYBACK_TRACE='1',BURRATA_DIRECT_YOUTUBE_SOURCE_FIXTURE=json.dumps(fixture,ensure_ascii=False))
  with log.open('wb') as f:
   child=subprocess.Popen([str(binary),'--exact','playback_sources::direct_youtube_integration_tests::live_direct_youtube_foreground_source_fixture_pcm','--ignored','--nocapture','--test-threads=1'],env=env,stdout=f,stderr=subprocess.STDOUT,start_new_session=True)
   try: row['exitCode']=child.wait(timeout=min(25,max(.01,deadline-time.monotonic())))
   except subprocess.TimeoutExpired:os.killpg(child.pid,signal.SIGKILL);child.wait();row.update(exitCode=child.returncode,failure='OWNED_GROUP_TIMEOUT')
  results=[]
  for line in log.read_text().splitlines():
   if line.startswith('{'):
    try:
     r=json.loads(line)
     if r.get('experiment')=='production-direct-youtube-foreground-v1':results.append(r)
    except ValueError:pass
  row['logSha256']=sha(log)
  if len(results)==1:row.update(result=results[0],ok=row['exitCode']==0 and results[0].get('ok') is True)
  else:row['failure']='MISSING_OR_DUPLICATE_RESULT'
  (out/'report.json').write_text(json.dumps(report,indent=2)+'\n')
  print(json.dumps(row),flush=True)
report['summary']={'planned':18,'started':sum(r['started'] for r in report['rows']),'passed':sum(r['ok'] for r in report['rows']),'failed':sum(r['started'] and not r['ok'] for r in report['rows'])}
(out/'report.json').write_text(json.dumps(report,indent=2)+'\n');print(json.dumps(report['summary']),flush=True)
