import {Client} from '@modelcontextprotocol/client';
import {StdioClientTransport} from '@modelcontextprotocol/client/stdio';
import {assessNativeOutput,assessNativeProgress,assessPrivateLinuxRoute,unmuteVerifiedPrivateOutput,verifyRemuteWhilePlaying} from './native-audio-protocol.mjs';
import fs from 'node:fs';
import path from 'node:path';
import {fileURLToPath} from 'node:url';
import {execFileSync,spawn} from 'node:child_process';
const sleep=ms=>new Promise(resolve=>setTimeout(resolve,ms));

function procIdentity(pid) {
  if (!Number.isSafeInteger(pid) || pid<=0) throw new Error('Invalid owned process ID');
  const root=`/proc/${pid}`, fields=fs.readFileSync(`${root}/stat`,'utf8').split(') ')[1].trim().split(/\s+/);
  return {pid,state:fields[0],pgid:Number(fields[2]),startTicks:fields[19],exe:fs.readlinkSync(`${root}/exe`),uid:fs.statSync(root).uid};
}
export function collectPrivateFacts(config) {
  if (process.platform!=='linux') throw new Error('Private monitor is Linux only');
  const json=kind=>JSON.parse(execFileSync(config.pactl,['--server',`unix:${config.socket}`,'--format=json','list',kind],{env:process.env,timeout:2000,maxBuffer:1024*1024,stdio:['ignore','pipe','pipe']}).toString());
  const inputs=json('sink-inputs');
  const pid=Number(inputs[0]?.properties?.['application.process.id']);
  const socket=fs.lstatSync(config.socket),parent=path.dirname(config.socket);
  return {platform:process.platform,envServer:process.env.PULSE_SERVER,envSink:process.env.PULSE_SINK,
    socket:{isSocket:socket.isSocket(),uid:socket.uid,canonical:fs.realpathSync(parent)===parent,parentMode:fs.statSync(parent).mode&0o777},
    pulse:procIdentity(config.pulse.pid),modules:json('modules'),sinks:json('sinks'),inputs,app:procIdentity(pid)};
}

/** Own only a bounded private-source recorder; injectable spawn enables no-device tests. */
export function startBoundedMonitor(config,{spawnProcess=spawn,maxBytes=44100*2*4*10,timeoutMs=10000,stopGraceMs=2000,closeFile=fs.closeSync}={}) {
  if (config.recordFile!==path.join(config.tempRoot,'native-monitor.f32le')) throw new Error('Monitor destination must be smoke-owned');
  const fd=fs.openSync(config.recordFile,'wx',0o600);
  let child,bytes=0,error=null,stopping=false,closed=false,fdClosed=false,acceptData=true,stopPromise,rejectFailure;
  const failure=new Promise((_,reject)=>{rejectFailure=reject;});failure.catch(()=>{});
  const closeFd=()=>{if(!fdClosed){closeFile(fd);fdClosed=true;}};
  let resolveClosed;const closePromise=new Promise(resolve=>{resolveClosed=resolve;});
  const fail=message=>{acceptData=false;if(!error){error=new Error(message);rejectFailure(error);}if(child&&!closed)child.kill('SIGTERM');};
  try {
    child=spawnProcess(config.parec,[`--server=unix:${config.socket}`,'--record','--device=burrata_smoke.monitor','--raw','--format=float32le','--rate=44100','--channels=2','--latency-msec=50','--process-time-msec=10'],{env:process.env,stdio:['ignore','pipe','pipe']});
  } catch(e){closeFd();throw e;}
  child.stdout.on('data',chunk=>{if(!acceptData||closed||fdClosed)return;if(bytes+chunk.length>maxBytes){fail('Private monitor exceeded byte bound');return;}try{fs.writeSync(fd,chunk);bytes+=chunk.length;}catch{fail('Private monitor write failed');}});
  child.stderr?.on('data',()=>{});
  child.on('error',()=>{fail('Private monitor could not start');});
  child.on('close',()=>{closed=true;closeFd();resolveClosed();if(!stopping&&!error)fail('Private monitor ended before completion');});
  const timer=setTimeout(()=>fail('Private monitor exceeded time bound'),timeoutMs);timer.unref?.();
  return {failure,assertHealthy(){if(error)throw error;},get bytes(){return bytes;},
    stop(){
      if(stopPromise)return stopPromise;
      stopping=true;acceptData=false;clearTimeout(timer);
      stopPromise=(async()=>{
        const waitClose=async()=>{let timer;try{return await Promise.race([closePromise.then(()=>true),new Promise(resolve=>{timer=setTimeout(()=>resolve(false),stopGraceMs);})]);}finally{clearTimeout(timer);}};
        if(!closed){child.kill('SIGTERM');if(!await waitClose()){child.kill('SIGKILL');if(!await waitClose())throw new Error('Owned private recorder did not close after SIGKILL');}}
        // Only the child close event closes the descriptor: no late stdout writes or early success.
        if(!closed||!fdClosed)throw new Error('Owned private recorder closure not proven');
        if(error)throw error;return bytes;
      })();return stopPromise;
    }};
}

export async function main(args=process.argv.slice(2)) {
 const [command,discoveryPath,trackId,configPath]=args;
 if(!command||!discoveryPath||!/^local:[a-f0-9]{64}$/.test(trackId??''))throw new Error('usage: check-installed-native-audio.mjs MCP_COMMAND AUTOMATION_FILE LOCAL_FIXTURE_ID [PRIVATE_LINUX_CONFIG]');
 const config=configPath?JSON.parse(fs.readFileSync(configPath,'utf8')):null;
 if(config&&process.platform!=='linux')throw new Error('Private unmute/capture is prohibited outside Linux');
 const client=new Client({name:'burrata-installed-native-audio-smoke',version:'1.0.0'});
 const transport=new StdioClientTransport({command,args:[],env:{...process.env,BURRATA_AUTOMATION_FILE:discoveryPath},stderr:'pipe'});transport.stderr?.on('data',()=>{});
 async function call(name,args={},timeout=65000){const result=await client.callTool({name,arguments:args},{timeout});if(result.isError)throw new Error(`Native audio tool failed: ${name}`);return result.structuredContent?.result??result.structuredContent;}
 let monitor,result,unmuteAt,unmuteOffset,remuteOffset,route,remuteProof;
 const bytesPerSecond=44100*2*4;
 const recordedUntil=async seconds=>{while(monitor.bytes<seconds*bytesPerSecond)await checked(sleep(25));};
 const checked=promise=>monitor?Promise.race([promise,monitor.failure]):promise;
 try {
  await client.connect(transport);const tools=await client.listTools();
  for(const name of ['play_tracks','pause_playback','inspect_native_output','set_volume'])if(!tools.tools.some(t=>t.name===name))throw new Error(`Required Native smoke tool missing: ${name}`);
  const readyDeadline=performance.now()+30000;let ready=false;
  while(performance.now()<readyDeadline){try{await call('set_volume',{volume:0},3000);ready=true;break;}catch{await sleep(200);}}
  if(!ready)throw new Error('Native session did not finish restoring playback ownership');
  await call('set_volume',{volume:0});await call('play_tracks',{trackIds:[trackId]});
  const deadline=performance.now()+30000;let first;
  while(performance.now()<deadline){try{const sample=await checked(call('inspect_native_output'));assessNativeOutput(sample,trackId);first=sample;break;}catch(error){monitor?.assertHealthy();await checked(sleep(200));}}
  if(!first)throw new Error('Native output fixture did not reach the mixer');
  // Establish healthy source PCM while MUTED before starting the separate OS phase.
  if(config){
   const routeDeadline=performance.now()+5000;let facts;
   while(performance.now()<routeDeadline){try{facts=collectPrivateFacts(config);assessPrivateLinuxRoute(config,facts);break;}catch{facts=null;await sleep(100);}}
   if(!facts)throw new Error('Owned private app route unavailable; never unmuted');
   monitor=startBoundedMonitor(config);await recordedUntil(.3);
   // Revalidate immediately before the only volume1 call.
   route=await unmuteVerifiedPrivateOutput(config,collectPrivateFacts(config),call);unmuteAt=performance.now();unmuteOffset=monitor.bytes/bytesPerSecond;
  }
  await checked(sleep(config?Math.max(1100,3800-(performance.now()-unmuteAt)):1100));
  if(config)await recordedUntil(unmuteOffset+3.8);
  const second=await checked(call('inspect_native_output'));
  if(config){assessPrivateLinuxRoute(config,collectPrivateFacts(config));await checked(call('set_volume',{volume:0}));remuteOffset=monitor.bytes/bytesPerSecond;remuteProof=await verifyRemuteWhilePlaying(second,trackId,()=>recordedUntil(remuteOffset+1.0),()=>checked(call('inspect_native_output')),()=>checked(call('pause_playback')));await monitor.stop();}
  const evidence=assessNativeOutput(second,trackId);assessNativeProgress(first,second);
  result={status:'sampled',...evidence,clockAdvanceSeconds:second.position-first.position};
  if(config)result.privateMonitorControl={...route,sampleRate:44100,channels:2,bytes:monitor.bytes,unmuteOffsetSeconds:unmuteOffset,remuteOffsetSeconds:remuteOffset,postPlayerGain:true,...remuteProof};
 }catch(error){result={status:'failed',error:error.message};process.exitCode=1;}
 finally {await call('set_volume',{volume:0},3000).catch(()=>{});await call('pause_playback',{},3000).catch(()=>{});if(monitor)await monitor.stop().catch(()=>{process.exitCode=1;result={status:'failed',error:'Private monitor cleanup or capture failed'};});await client.close().catch(()=>{});}
 process.stdout.write(JSON.stringify(result)+'\n');return result;
}
if(process.argv[1]&&path.resolve(process.argv[1])===fileURLToPath(import.meta.url))await main();
