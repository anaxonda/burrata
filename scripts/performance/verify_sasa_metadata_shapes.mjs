// One live check per public metadata shape; not a latency benchmark.
import {Client} from '@modelcontextprotocol/client';
import {StdioClientTransport} from '@modelcontextprotocol/client/stdio';
import {createHash} from 'node:crypto';
import {readFile,mkdir,writeFile} from 'node:fs/promises';
import {validateSasaManifest} from './sasa_qualification_manifest.mjs';
import {createSustainedPlaybackCheck} from './sustained_playback.mjs';
const [manifestFile,outputDirectory]=process.argv.slice(2);
if(!manifestFile||!outputDirectory)throw Error('Usage: verify_sasa_metadata_shapes.mjs MANIFEST OUTPUT_DIRECTORY [--transition-only]');
const {automationFile,binaryFile,fixture}=await validateSasaManifest(manifestFile);
await mkdir(outputDirectory,{recursive:false});
const client=new Client({name:'sasa-metadata-live-check',version:'1.0.0'});
const transport=new StdioClientTransport({command:'bash',args:['scripts/dev/mcp-stdio.sh'],cwd:process.cwd(),env:{...process.env,BURRATA_AUTOMATION_FILE:automationFile},stderr:'pipe'});
transport.stderr?.on('data',()=>{});
const sleep=ms=>new Promise(resolve=>setTimeout(resolve,ms));
async function call(name,args={},timeout=25000){const r=await client.callTool({name,arguments:args},{timeout});if(r.isError)throw Error('App command failed');return r.structuredContent?.result??r.structuredContent;}
function reasonCategory(reason){
 if(/^No playable audio match was found/.test(reason))return 'no_playable_recording_match';
 if(/timed out|deadline|startup budget/i.test(reason))return 'startup_deadline';
 if(/not found|not installed|executable/i.test(reason))return 'tooling_or_source_unavailable';
 if(/verification|sign in|login/i.test(reason))return 'provider_verification';
 return reason?'other_source_failure':null;
}
async function attempt(trackId,expectedDuration,kind){
 const before=await call('playback_state');const started=performance.now();let commandFailed=false,latest=null;
 const pending=call('play_tracks',{trackIds:[trackId]}).catch(()=>{commandFailed=true;});
 let outcome='observation_timeout';let reason='';
 const continued=createSustainedPlaybackCheck();let continuedPlayback=null;
 while(performance.now()-started<25000){
  latest=await call('playback_state',{},5000);const diagnostic=latest.diagnostics??{};
  if(latest.generation>before.generation&&diagnostic.lastSourceFailure?.trackId===trackId&&(JSON.stringify(diagnostic.lastSourceFailure)!==JSON.stringify(before.diagnostics?.lastSourceFailure)||['failed','error'].includes(latest.status))){outcome='unavailable';reason=String(diagnostic.lastSourceFailure.reason??latest.playerMessage??'');break;}
  if(latest.generation>before.generation&&latest.trackId===trackId&&diagnostic.operationId!==before.diagnostics?.operationId){
   continuedPlayback=continued.observe(latest,performance.now());
   if(continuedPlayback.status==='failed'){outcome='continued_playback_failed';break;}
   if(continuedPlayback.status==='ready'){outcome='playing';break;}
  }else if(continuedPlayback){outcome='continued_playback_session_changed';break;}
  await sleep(100);
 }
 await pending;const elapsedMs=Math.round(performance.now()-started);const diagnostic=latest?.diagnostics??{};
 const durationCorrect=outcome==='playing'?Math.abs(latest.duration-expectedDuration)<=1:null;
 const sourceTitle=String(latest?.candidate?.title??'');
 const wrongEdition=kind==='sasa'&&outcome==='playing'&&/radio edit|remix|rework|bootleg/i.test(sourceTitle);
 const category=reasonCategory(reason);
 const result={kind,decodedPlaybackPass:outcome==='playing'&&durationCorrect&&!wrongEdition,continuedPlayback,metadataShape:fixture.sasaCases.find(testCase=>testCase.trackId===trackId)?.metadataShape??(kind==='remote'?'known_remote_control':'synthetic_local_control'),outcome,elapsedMs,within20Seconds:elapsedMs<=20000,playCommandReportedError:commandFailed,durationCorrect,expectedDurationSeconds:expectedDuration,actualDurationSeconds:Number.isFinite(latest?.duration)?latest.duration:null,durationDifferenceSeconds:Number.isFinite(latest?.duration)?latest.duration-expectedDuration:null,wrongEdition,reasonCategory:category,visibleReason:category==='no_playable_recording_match'&&!/https?:|Bearer|cookie/i.test(reason)?reason.slice(0,700):null,engine:latest?.engine,sourceProvider:['monochrome','youtube','soundcloud','local'].includes(latest?.candidate?.provider)?latest.candidate.provider:null,firstAudioMs:diagnostic.firstAudioMs??null,sourceReadyMs:diagnostic.sourceReadyMs??null,operationId:diagnostic.operationId??null,commandSerializedBytes:diagnostic.commandSerializedBytes??null};
 result.qualificationPass=result.decodedPlaybackPass&&!commandFailed&&result.within20Seconds&&result.engine==='native'&&result.operationId?.startsWith('native-')&&result.commandSerializedBytes>0;
 result.unavailableReasonVerified=outcome==='unavailable'&&category!==null&&result.within20Seconds;
 await call('pause_playback');return result;
}
async function transitionControl(){
 const before=await call('playback_state');if(before.repeat!=='off'||before.shuffle)throw Error('Control requires ordinary queue order');
 await call('play_tracks',{trackIds:[fixture.trackId,fixture.trackId]});
 let startup=await call('playback_state');const deadline=performance.now()+10000;
 while((startup.status!=='playing'||startup.currentIndex!==0)&&performance.now()<deadline){await sleep(100);startup=await call('playback_state');}
 const startupId=startup.diagnostics?.operationId;
 await call('seek_playback',{position:fixture.durationSeconds-0.5});const seek=await call('playback_state');const seekId=seek.diagnostics?.seekOperationId;
 let next=await call('playback_state');
 const transitionDeadline=performance.now()+15000;
 const continued=createSustainedPlaybackCheck();let continuedPlayback=null;
 while(performance.now()<transitionDeadline){
  if(next.currentIndex===1&&next.trackId===fixture.trackId){
   continuedPlayback=continued.observe(next,performance.now());
   if(continuedPlayback.status==='ready'||continuedPlayback.status==='failed')break;
  }else if(continuedPlayback){continuedPlayback={status:'failed',failureCategory:'continued_playback_session_changed'};break;}
  await sleep(100);next=await call('playback_state',{},5000);
 }
 const transitionId=next.diagnostics?.operationId;
 const decodedPlaybackPass=continuedPlayback?.status==='ready';
 const verified=decodedPlaybackPass&&next.currentIndex===1&&next.status==='playing'&&next.diagnostics?.operationKind==='transition'&&!!startupId&&!!seekId&&!!transitionId&&new Set([startupId,seekId,transitionId]).size===3;
 await call('pause_playback');return {kind:'operation_ids',outcome:verified?'verified':'failed',decodedPlaybackPass,continuedPlayback,startupBudgetMs:10000,transitionBudgetMs:15000,startupId,seekId,transitionId,seekCommandSerializedBytes:seek.diagnostics?.seekCommandSerializedBytes??null,seekStatus:seek.status,seekPosition:seek.position,seekFirstAudioMs:next.diagnostics?.seekFirstAudioMs??null,seekOperationFirstAudioMs:next.diagnostics?.seekOperationFirstAudioMs??null,finalStatus:next.status,finalPosition:next.position,finalIndex:next.currentIndex,queueCount:next.queue?.length??null,verified};
}
const transitionOnly=process.argv.includes('--transition-only');
const results=[];
try{
 await client.connect(transport);await call('set_volume',{volume:0});
 if(!transitionOnly){
 results.push(await attempt(fixture.trackId,fixture.durationSeconds,'local'));
 for(const testCase of fixture.sasaCases)results.push(await attempt(testCase.trackId,testCase.expectedDurationSeconds,'sasa'));
 results.push(await attempt(fixture.remoteCase.trackId,fixture.remoteCase.expectedDurationSeconds,'remote'));
 results.push(await attempt(fixture.trackId,fixture.durationSeconds,'local'));
 }
 results.push(await transitionControl());
 if(results.some(result=>result.kind==='operation_ids'?!result.verified:!result.qualificationPass))process.exitCode=1;
}catch{results.push({outcome:'probe_error',reason:'App-specific verification did not finish; raw tool output was not retained.'});process.exitCode=1;}
finally{
 await call('pause_playback',{},5000).catch(()=>{});
 await client.close().catch(()=>{});
 const report={decodedPlaybackPass:results.length>0&&results.every(result=>result.decodedPlaybackPass===true),qualificationPass:!transitionOnly&&results.every(result=>result.kind==='operation_ids'?result.verified:result.qualificationPass===true),failureCount:results.filter(result=>result.kind==='operation_ids'?!result.verified:result.qualificationPass!==true).length,scope:transitionOnly?'Muted local duplicate-occurrence transition control only; no statistical qualification.':'One live request per metadata shape plus muted local controls; no statistical qualification.',binarySha256:createHash('sha256').update(await readFile(binaryFile)).digest('hex'),providerOrder:['monochrome','youtube','soundcloud'],results};
 await writeFile(`${outputDirectory}/report.json`,JSON.stringify(report,null,2)+'\n',{mode:0o600,flag:'wx'});console.log(JSON.stringify(report));
}
