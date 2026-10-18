import { readFile, writeFile } from 'node:fs/promises';
import { Client } from '@modelcontextprotocol/client';
import { StdioClientTransport } from '@modelcontextprotocol/client/stdio';
import { performance } from 'node:perf_hooks';
import { strict as assert } from 'node:assert';
import { additionalPageBoundaryOffsets } from './library_page_matrix_boundaries.mjs';

const [automationFile, expectedCountText, outputFile, appPidText, ticksPerSecondText='100', fixtureMode='catalog', runMode='matrix', projectionState='initial'] = process.argv.slice(2);
const expectedCount = Number(expectedCountText), appPid = Number(appPidText), ticksPerSecond = Number(ticksPerSecondText);
assert(automationFile && outputFile && Number.isInteger(expectedCount) && expectedCount > 0 && Number.isInteger(appPid));
assert(['catalog','asset-backed'].includes(fixtureMode),'unknown fixture mode');
assert(['matrix','bpm-pages-once'].includes(runMode),'unknown run mode');
assert(['initial','ready'].includes(projectionState),'unknown projection state');
if(fixtureMode==='asset-backed'&&projectionState==='ready') {
  assert.equal(process.env.BURRATA_SORT_PROJECTION_READY,'1',
    'ready asset-backed timing requires the Python wrapper read-only projection readiness gate; MCP omits sortPending');
}
const client = new Client({name:'burrata-library-page-matrix',version:'1.0.0'});
const transport = new StdioClientTransport({command:'bash',args:['scripts/dev/mcp-stdio.sh'],cwd:process.cwd(),env:{...process.env,BURRATA_AUTOMATION_FILE:automationFile},stderr:'pipe'});
transport.stderr?.on('data',()=>{});
const appCpuTicks = async () => {
  const stat = await readFile(`/proc/${appPid}/stat`,'utf8');
  const close = stat.lastIndexOf(')');
  const f = stat.slice(close+2).trim().split(/\s+/);
  return Number(f[11])+Number(f[12]); // utime + stime, fields 14 and 15 overall.
};
const call = async (args) => {
  const started = performance.now();
  const result = await client.callTool({name:'list_tracks_page',arguments:args});
  const wallMs = performance.now()-started;
  assert(!result.isError,`list_tracks_page returned an error`);
  const body = result.structuredContent?.result ?? result.structuredContent;
  assert(body && Array.isArray(body.items),'missing items page');
  return {body,wallMs,ipcJsonBytes:Buffer.byteLength(JSON.stringify(body))};
};
function expected(count, field, direction, assets=false) {
  const indexes=Array.from({length:count},(_,i)=>i);
  const bucket=i=>{
    if(field==='title') return assets&&i%997===777?`Z Manual Override ${String(i).padStart(6,'0')}`:`Library Track ${String(Math.floor(i/10)).padStart(5,'0')}`;
    if(field==='artist') return `Artist Group ${String(i%23).padStart(2,'0')}`;
    if(field==='duration') return i===0?null:180+Math.floor(i/10);
    if((field==='bpm'||field==='plays')&&assets) {
      if(i%211===0) return 180+i%13;
      if(i%7!==0) return 60+i%43;
      if(i%14===0) return 70+i%37;
      return null;
    }
    return Math.floor(i/10);
  };
  indexes.sort((a,b)=>{
    const av=bucket(a), bv=bucket(b);
    if(av===null&&bv!==null)return 1; if(bv===null&&av!==null)return -1;
    const primary=av===bv?0:(av<bv?-1:1)*(direction==='desc'?-1:1);
    return primary || (b-a); // stable source order is newest/rowid-desc on ties.
  });
  return indexes.map(i=>`fixture:matrix-${String(i).padStart(6,'0')}`);
}
const scenarios=[
  {name:'title_asc',args:{sortField:'title',direction:'asc'},ids:expected(expectedCount,'title','asc',fixtureMode==='asset-backed')},
  {name:'artist_asc',args:{sortField:'artist',direction:'asc'},ids:expected(expectedCount,'artist','asc')},
  {name:'duration_asc',args:{sortField:'duration',direction:'asc'},ids:expected(expectedCount,'duration','asc')},
  {name:'duration_desc',args:{sortField:'duration',direction:'desc'},ids:expected(expectedCount,'duration','desc')},
  {name:'unicode_query_source_order',args:{query:'αθήνα',sortField:'sourceOrder',direction:'asc'},ids:Array.from({length:expectedCount},(_,i)=>i).filter(i=>i%997===777).sort((a,b)=>b-a).map(i=>`fixture:matrix-${String(i).padStart(6,'0')}`)},
];
if(fixtureMode==='asset-backed') {
  scenarios.splice(1,0,
    {name:'effective_bpm_asc',args:{sortField:'bpm',direction:'asc'},ids:expected(expectedCount,'bpm','asc',true)},
    {name:'effective_bpm_desc',args:{sortField:'bpm',direction:'desc'},ids:expected(expectedCount,'bpm','desc',true)});
}
const cpuHz=ticksPerSecond;
const report={schemaVersion:1,workload:'actual_burrata_mcp_library_page_matrix',count:expectedCount,fixtureMode,runMode,projectionState,tool:'list_tracks_page',warmups:3,repetitions:20,pageSize:200,scenarios:[],limitations:['CPU is the Burrata executable user+system CPU ticks sampled from /proc; includes only time around each MCP call.','IPC bytes are UTF-8 serialized JSON result size, not framed transport bytes.','Per-read CPU is sampled at process tick resolution and can be zero for short calls; aggregate CPU also includes inter-read validation and sampling overhead.','The isolated fixture is synthetic and does not represent a user library.']};
if(fixtureMode==='asset-backed') report.fixture={kind:'synthetic tiny WAV local_assets',fileCount:expectedCount,uniqueContentCount:1,storage:'one tiny valid WAV inode with isolated per-recording hardlinks',completedCurrentVersionAnalysisCount:expectedCount,expectedAnalysisBpmCount:Math.ceil(expectedCount/14),staleHashAnalysisCount:Math.floor((expectedCount-1)/7)-Math.floor((expectedCount-1)/14),analysisState:'steady-state synthetic completed rows at the app-pinned analysis version; no startup analysis contention qualification',manualTitleCount:Array.from({length:expectedCount},(_,i)=>i).filter(i=>i%997===777).length,manualBpmCount:Math.floor((expectedCount-1)/211)+1,audioQualification:false};
try {
  await client.connect(transport,{timeout:20_000});
  if(runMode==='bpm-pages-once') {
    assert(fixtureMode==='asset-backed','bpm-pages-once requires the asset-backed fixture');
    const args={view:'library',limit:200,sortField:'bpm',direction:'asc'};
    const first=await call(args);
    const continuation=await call({...args,offset:200});
    const expectedIds=expected(expectedCount,'bpm','asc',true);
    const firstExpected=expectedIds.slice(0,200);
    const continuationExpected=expectedIds.slice(200,400);
    const firstIds=first.body.items.map(row=>row.id);
    const continuationIds=continuation.body.items.map(row=>row.id);
    const mismatches=(actual,wanted,offset)=>Array.from({length:Math.max(actual.length,wanted.length)},(_,i)=>({
      absoluteIndex:offset+i,expectedId:wanted[i]??null,actualId:actual[i]??null
    })).filter(row=>row.expectedId!==row.actualId);
    report.probe={scenario:'effective_bpm_asc',total:first.body.total,firstPage:{offset:0,count:firstIds.length,ids:firstIds},
      continuation:{offset:200,count:continuationIds.length,ids:continuationIds},expected:{firstPageIds:firstExpected,continuationIds:continuationExpected},
      firstPageMismatches:mismatches(firstIds,firstExpected,0),continuationMismatches:mismatches(continuationIds,continuationExpected,200),
      firstHasMore:first.body.hasMore,continuationHasMore:continuation.body.hasMore};
    assert.equal(first.body.total,expectedIds.length,'probe total mismatch');
    assert.deepEqual(firstIds,firstExpected,'probe first-page identity/order mismatch');
    assert.deepEqual(continuationIds,continuationExpected,'probe continuation identity/order mismatch');
    await writeFile(outputFile,JSON.stringify(report,null,2)+'\n');
    process.stdout.write(JSON.stringify({complete:true,probe:'effective_bpm_asc',failures:0}));
  } else {
  for(const scenario of scenarios){
    const args={view:'library',limit:200,offset:0,...scenario.args};
    let actualFirstPageIds=[];
    const sample=async()=>{
      const cpuBefore=await appCpuTicks();
      const {body,wallMs,ipcJsonBytes}=await call(args);
      const cpuAfter=await appCpuTicks();
      assert(Number.isSafeInteger(cpuBefore)&&Number.isSafeInteger(cpuAfter)&&cpuAfter>=cpuBefore,'proc CPU tick sample invalid');
      const ids=body.items.map(row=>row.id);
      actualFirstPageIds=ids;
      assert.equal(body.total,scenario.ids.length,`${scenario.name}: total mismatch`);
      assert.equal(body.hasMore,body.total>args.offset+args.limit,`${scenario.name}: hasMore mismatch`);
      assert.deepEqual(ids,scenario.ids.slice(args.offset,args.offset+args.limit),`${scenario.name}: numeric identity/order mismatch`);
      return {wallMs,ipcJsonBytes,appCpuTicks:cpuAfter-cpuBefore,appCpuMs:(cpuAfter-cpuBefore)/cpuHz*1000};
    };
    // In ready mode the Python wrapper gates on projection rows read-only.
    // Initial mode deliberately retains startup/fallback latency in its samples.
    await sample();
    for(let i=1;i<3;i++) await sample();
    const cpuStart=await appCpuTicks();
    const results=[];
    // Retain completed samples even if a later read or parity check fails.
    report.activeScenarioSamples={name:scenario.name,samples:results};
    for(let i=0;i<20;i++) results.push({read:i+1,...await sample()});
    const cpuEnd=await appCpuTicks();
    const continuation=await call({...args,offset:200});
    const continuationIds=continuation.body.items.map(row=>row.id);
    const expectedContinuationIds=scenario.ids.slice(200,400);
    const continuationMismatches=Array.from({length:Math.max(continuationIds.length,expectedContinuationIds.length)},(_,i)=>({absoluteIndex:200+i,expectedId:expectedContinuationIds[i]??null,actualId:continuationIds[i]??null})).filter(row=>row.expectedId!==row.actualId);
    if(continuationMismatches.length) report.scenarios.push({name:scenario.name,total:scenario.ids.length,failures:1,continuationVerified:false,continuationDiagnostic:{offset:200,expectedCount:expectedContinuationIds.length,actualCount:continuationIds.length,mismatches:continuationMismatches,firstPageIds:actualFirstPageIds,expectedContinuationIds,continuationIds}});
    assert.deepEqual(continuationIds,expectedContinuationIds,`${scenario.name}: continuation identity/order mismatch`);
    assert.equal(new Set([...scenario.ids.slice(0,200),...continuationIds]).size,Math.min(scenario.ids.length,400),`${scenario.name}: duplicate occurrence across page boundary`);
    assert.equal(continuation.body.total,scenario.ids.length);
    const boundaryChecks=[];
    const verifiedIds=new Set([...actualFirstPageIds,...continuationIds]);
    for(const offset of additionalPageBoundaryOffsets(scenario.ids.length,200,[0,200])) {
      report.activeBoundaryCheck={name:scenario.name,offset,expectedCount:Math.min(200,scenario.ids.length-offset)};
      const boundary=await call({...args,offset});
      const ids=boundary.body.items.map(row=>row.id);
      assert.equal(boundary.body.total,scenario.ids.length,`${scenario.name}: boundary total mismatch at offset ${offset}`);
      assert.equal(boundary.body.hasMore,scenario.ids.length>offset+200,`${scenario.name}: boundary hasMore mismatch at offset ${offset}`);
      assert.deepEqual(ids,scenario.ids.slice(offset,offset+200),`${scenario.name}: boundary identity/order mismatch at offset ${offset}`);
      for(const id of ids) {
        assert(!verifiedIds.has(id),`${scenario.name}: duplicate ID across sampled pages at offset ${offset}`);
        verifiedIds.add(id);
      }
      boundaryChecks.push({offset,count:ids.length,hasMore:boundary.body.hasMore});
      delete report.activeBoundaryCheck;
    }
    const sort=(xs)=>xs.slice().sort((a,b)=>a-b);
    const metric=(key)=>{const xs=sort(results.map(r=>r[key]));const middle=xs.length/2;return {median:(xs[middle-1]+xs[middle])/2,p95:xs[Math.ceil(xs.length*.95)-1],worst:xs.at(-1)}};
    report.scenarios.push({name:scenario.name,total:scenario.ids.length,reads:results.length,failures:0,wallMs:metric('wallMs'),appCpuMs:{aggregateForReads:results.length,total:(cpuEnd-cpuStart)/cpuHz*1000,perRead:(cpuEnd-cpuStart)/cpuHz*1000/results.length},ipcJsonBytes:metric('ipcJsonBytes'),samples:results,perReadAppCpuMs:metric('appCpuMs'),continuationVerified:true,boundaryChecks});
    delete report.activeScenarioSamples;
  }
  await writeFile(outputFile,JSON.stringify(report,null,2)+'\n');
  process.stdout.write(JSON.stringify({complete:true,scenarios:report.scenarios.length,failures:0}));
  }
} catch(error) {
  const message=String(error?.message??'');
  report.failureCount=1;
  report.failureCategory=/total mismatch/i.test(message)?'total_mismatch':/hasMore mismatch/i.test(message)?'has_more_mismatch':/continuation/i.test(message)?'continuation_mismatch':/numeric identity|order mismatch/i.test(message)?'identity_or_order_mismatch':/proc/i.test(message)?'app_cpu_sample_unavailable':'mcp_or_matrix_error';
  report.failedScenario=runMode==='bpm-pages-once'?'effective_bpm_asc':report.scenarios.length < scenarios.length ? scenarios[report.scenarios.length].name : 'post_matrix';
  await writeFile(outputFile,JSON.stringify(report,null,2)+'\n');
  process.stdout.write(JSON.stringify({complete:false,failureCategory:report.failureCategory,failures:1}));
  process.exitCode=1;
} finally { await client.close().catch(()=>{}); }
