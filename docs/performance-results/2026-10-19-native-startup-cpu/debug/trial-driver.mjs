import {Client} from '@modelcontextprotocol/client';
import {StdioClientTransport} from '@modelcontextprotocol/client/stdio';
import {readFile,writeFile,mkdir} from 'node:fs/promises';
import {spawn} from 'node:child_process';
import {dirname,join} from 'node:path';
const [manifestFile,index,trackId]=process.argv.slice(2);
if(!manifestFile||!/^\d+$/.test(index)||!trackId)throw new Error('INVALID_ARGUMENTS');
const manifest=JSON.parse(await readFile(manifestFile,'utf8'));
if(!/^\/tmp\/burrata-launch\.[A-Za-z0-9_-]+$/.test(manifest.profile)||manifest.automationFile!==join(manifest.profile,'data/org.burrata.discovery/automation.json')||await readFile(join(manifest.profile,'.burrata-isolated-profile'),'utf8')!=='burrata isolated memory run\n')throw new Error('ISOLATED_PROFILE_REQUIRED');
if(![manifest.localTrackId,...manifest.publicTrackIds].includes(trackId))throw new Error('FIXTURE_TRACK_REQUIRED');
const out='/tmp/burrata-native-cpu-current';await mkdir(out,{recursive:true});
const report={schema:'burrata.native-cpu-trial/v1',trialIndex:Number(index),trackId,binarySha256:manifest.sha256,scope:'Muted isolated-profile debug native output; 250ms MCP polling; sampler/client/Vite CPU excluded; no physical audibility.',markers:{},states:[]};
const c=new Client({name:'burrata-native-cpu-trial',version:'1'});
const transport=new StdioClientTransport({command:'bash',args:['scripts/dev/mcp-stdio.sh'],cwd:process.cwd(),env:{...process.env,BURRATA_AUTOMATION_FILE:manifest.automationFile},stderr:'pipe'});transport.stderr?.on('data',()=>{});
const sleep=ms=>new Promise(r=>setTimeout(r,ms));
async function call(name,args={}){const r=await c.callTool({name,arguments:args},{timeout:20000});if(r.isError)throw new Error('MCP_COMMAND_FAILED');return r.structuredContent?.result??r.structuredContent;}
function safe(s){return {atUnixMs:Date.now(),engine:s.engine,status:s.status,position:s.position,paused:s.paused,volume:s.volume,trackId:s.trackId,entryId:s.entryId,firstAudioMs:s.diagnostics?.firstAudioMs,sourceReadyMs:s.diagnostics?.sourceReadyMs,operationId:s.diagnostics?.operationId,decoderKind:s.diagnostics?.decoder?.decoderKind,failurePresent:!!s.failure};}
let sampler,samplerDone;
try{
 await c.connect(transport);let s=await call('playback_state',{offset:0,limit:5});
 if(s.engine!=='native'||s.volume!==0)throw new Error('MUTED_NATIVE_REQUIRED');
 if(s.trackId)await call('pause_playback');
 const samplePath=join(out,`trial-${index}.cpu.jsonl`);
 report.markers.samplerLaunchUnixMs=Date.now();
 sampler=spawn('python3',['/tmp/burrata-native-cpu-sampler.py','--state',manifestFile,'--output',samplePath,'--seconds','28','--interval','0.1'],{stdio:['ignore','ignore','pipe']});
 sampler.stderr.on('data',()=>{});samplerDone=new Promise((resolve,reject)=>{sampler.on('error',()=>reject(new Error('SAMPLER_SPAWN_FAILED')));sampler.on('close',code=>code===0?resolve():reject(new Error('SAMPLER_FAILED')));});samplerDone.catch(()=>{});
 let ready=false;for(let i=0;i<50;i++){try{const first=(await readFile(samplePath,'utf8')).split('\n')[0];ready=JSON.parse(first).record_type==='capture_start';}catch{}if(ready)break;await sleep(50);}if(!ready)throw new Error('SAMPLER_NOT_READY');
 await sleep(1000);
 report.markers.commandSentUnixMs=Date.now();await call('play_tracks',{trackIds:[trackId]});report.markers.commandReturnedUnixMs=Date.now();
 const until=report.markers.commandSentUnixMs+24000;
 while(Date.now()<until){s=await call('playback_state',{offset:0,limit:5});const row=safe(s);report.states.push(row);if(row.trackId===trackId&&row.status==='playing'&&!report.markers.firstObservedPlayingUnixMs){report.markers.firstObservedPlayingUnixMs=row.atUnixMs;report.startup=row;}if(row.status==='failed')throw new Error('PLAYBACK_FAILED');await sleep(250);}
 report.markers.pauseSentUnixMs=Date.now();await call('pause_playback');report.final=safe(await call('playback_state',{offset:0,limit:5}));
 await samplerDone;report.markers.captureFinishedUnixMs=Date.now();
 report.ok=!!report.startup&&report.states.some(r=>r.position>=15)&&report.final.paused===true&&report.states.every(r=>r.volume===0);
 if(!report.ok)throw new Error('PROGRESSION_FAILED');
}catch(e){report.ok=false;report.failure=e.message;if(sampler&&!sampler.killed)sampler.kill('SIGTERM');process.exitCode=1;await call('pause_playback').catch(()=>{});}finally{await c.close().catch(()=>{});await writeFile(join(out,`trial-${index}.json`),JSON.stringify(report,null,2)+'\n',{flag:'wx'});console.log(JSON.stringify({trialIndex:index,ok:report.ok,failure:report.failure,firstAudioMs:report.startup?.firstAudioMs,sourceReadyMs:report.startup?.sourceReadyMs,states:report.states.length}));}
