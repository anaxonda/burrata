// Control only the disposable profile created by native_runtime_capture.py.
// Monitor PCM is assessed independently by the Python controller.
import {readFile, realpath, writeFile} from 'node:fs/promises';
import {execFile} from 'node:child_process';
import {promisify} from 'node:util';
import {auditionQueueMetadataRetained} from './source_audition_gates.mjs';
import {Client} from '@modelcontextprotocol/client';
import {StdioClientTransport} from '@modelcontextprotocol/client/stdio';

const [profile, mcpCommand, mode] = process.argv.slice(2);
if (!/^\/tmp\/burrata-launch\.[A-Za-z0-9_-]+$/.test(profile) || await realpath(profile) !== profile
    || await readFile(`${profile}/.burrata-isolated-profile`, 'utf8') !== 'burrata isolated memory run\n') {
  throw Error('Disposable profile required');
}
const spec = JSON.parse(await readFile(`${profile}/fixture/runtime-cases.json`, 'utf8'));
const client = new Client({name:'native-runtime-capture',version:'1.0.0'});
const transport = new StdioClientTransport({command:mcpCommand,args:[],
  env:{...process.env,BURRATA_AUTOMATION_FILE:`${profile}/data/org.burrata.discovery/automation.json`},stderr:'pipe'});
transport.stderr?.on('data',()=>{});
const sleep = ms => new Promise(resolve=>setTimeout(resolve,ms));
const report = {mode, cases:[], samples:[]};
async function call(name,args={},timeout=45000) {
  const r = await client.callTool({name,arguments:args},{timeout});
  if (r.isError) throw Error(`${name}: ${JSON.stringify(r.structuredContent?.error ?? r.content?.[0]?.text)}`);
  const body=r.structuredContent;
  return body && Object.hasOwn(body,"result") ? body.result : body;
}
async function captureUi(name) {
  const r=await client.callTool({name:'capture_main_ui',arguments:{}},{timeout:10000});
  const picture=r.content?.find(item=>item.type==='image');
  if(!picture?.data)throw Error('App-owned UI capture unavailable');
  const path=`${profile}/${name}.png`;await writeFile(path,Buffer.from(picture.data,'base64'));return path;
}
async function state() { return call('playback_state',{offset:0,limit:10}); }
function safe(s) {
  return {generation:s.generation,trackId:s.trackId,entryId:s.entryId,entryIds:s.queueEntryIds,index:s.currentIndex,
    status:s.status,position:s.position,duration:s.duration,catalogDuration:s.track?.durationSeconds,paused:s.paused,transition:s.transition,
    firstAudioMs:s.diagnostics?.firstAudioMs,recoveryCount:s.diagnostics?.recoveryCount,bufferingCount:s.diagnostics?.bufferingCount,seekSerial:s.seekSerial,
    message:s.playerMessage,loudnessNormalization:s.loudnessNormalization,candidate:s.candidate ? {provider:s.candidate.provider,id:s.candidate.id,title:s.candidate.title}:null};
}
try {
  await client.connect(transport);
  let ready=false;
  for (let i=0;i<120;i++) {
    try {
      if(mode==='migration'){const s=await state();if(JSON.stringify(s.queueEntryIds)!==JSON.stringify(['legacy-first','legacy-second'])){await sleep(250);continue;}}
      else await call('set_volume',{volume:1},3000);
      ready=true;break;
    } catch {await sleep(250);}
  }
  if (!ready) throw Error('Native session restoration did not finish');
  for(let i=0;i<20;i++){
    try{await call('inspect_playback_layout',{},3000);report.layoutDiagnostic={passed:true};break;}
    catch(error){report.layoutDiagnostic={passed:false,error:error.message};if(!error.message.includes('FRONTEND_NOT_READY'))break;await sleep(250);}
  }
  if(mode!=='migration')await call('set_volume',{volume:1});
if(mode==='source-audition'){
    // X11 interaction is restricted to this harness's verified private Xvfb
    // and this exact app process. Never targets the normal desktop/display.
    const appPid=process.env.BURRATA_QUAL_PRIVATE_APP_PID,xPid=process.env.BURRATA_QUAL_PRIVATE_XVFB_PID;
    const appExe=process.env.BURRATA_QUAL_PRIVATE_APP_EXE,display=process.env.DISPLAY;
    if(!/^\d+$/.test(appPid??'')||!/^\d+$/.test(xPid??'')||!/^:(?:9\d|10\d|11\d)$/.test(display??''))throw Error('Verified private display required');
    if(await realpath(`/proc/${appPid}/exe`)!==await realpath(appExe))throw Error('Private app owner changed');
    const xvfb=(await readFile(`/proc/${xPid}/cmdline`,'utf8')).split('\0');
    if(!xvfb[0].endsWith('/burrata-private-xvfb/root/usr/bin/Xvfb')||!xvfb.includes(display)||!xvfb.includes('-nolisten'))throw Error('Private Xvfb owner changed');
    const exec=promisify(execFile),matches=s=>s.trackId===spec.incidents[0].id&&s.candidate?.provider==='youtube'&&s.candidate.id==='youtube:rpCswezRrLs';
    const capture=captureUi;
    report.localStartedAtUnixMs=Date.now();
    await call('play_tracks',{trackIds:[spec.incidents[0].id]});await sleep(1300);
    const local=await state();await call('pause_playback');report.localPausedAtUnixMs=Date.now();
    const metadataBefore=(await call('get_track',{trackId:spec.incidents[0].id})).track;
    const jobsBefore=await call('download_jobs',{offset:0,limit:1});
    await call('navigate_ui',{view:'library'});await call('resize_app_window',{width:1200,height:850});await sleep(500);
    const windows=(await exec('xdotool',['search','--onlyvisible','--pid',appPid],{timeout:3000})).stdout.trim().split(/\s+/).filter(Boolean);
    if(windows.length!==1||!/^\d+$/.test(windows[0]))throw Error('A single owned private app window is required');
    const window=windows[0];let lastAction=0,anchor=null,readyToPause=false,finished=false,last=local;
    let screenshot=await capture('audition-ui-0');
    await writeFile(`${profile}/ui-step.json`,JSON.stringify({phase:'open-source-chooser',window,display,screenshot}));
    const start=performance.now();
    while(performance.now()-start<780000){
      let action=null;try{action=JSON.parse(await readFile(`${profile}/ui-action.json`,'utf8'));}catch{}
      if(action&&Number.isSafeInteger(action.id)&&action.id>lastAction){
        if(action.kind==='click'&&Number.isSafeInteger(action.x)&&action.x>=0&&action.x<1280&&Number.isSafeInteger(action.y)&&action.y>=0&&action.y<900&&[1,3].includes(action.button??1)){
          await exec('xdotool',['mousemove','--window',window,String(action.x),String(action.y),'click',String(action.button??1)],{timeout:3000});
        }else if(action.kind==='key'&&['Escape','Return','Tab','shift+Tab','Down','Up','Home','End'].includes(action.key)){
          await exec('xdotool',['windowfocus',window,'key','--clearmodifiers',action.key],{timeout:3000});
        }else if(action.kind==='abort'){throw Error('Private UI qualification aborted');}
        else if(action.kind!=='capture')throw Error('Invalid private UI test action');
        lastAction=action.id;await sleep(450);screenshot=await capture(`audition-ui-${lastAction}`);
        await writeFile(`${profile}/ui-step.json`,JSON.stringify({phase:readyToPause?'pause-preview':'select-and-preview-source',lastAction,window,display,screenshot,state:safe(await state())}));
      }
      last=await state();report.samples.push({ms:performance.now()-start,...safe(last)});
      if(matches(last)&&last.status==='playing'){
        if(anchor===null){anchor=last.position;report.auditionStartedAtUnixMs=Date.now();}
        if(!readyToPause&&last.position-anchor>=10){readyToPause=true;screenshot=await capture('audition-ui-ready-to-pause');await writeFile(`${profile}/ui-step.json`,JSON.stringify({phase:'pause-preview',lastAction,window,display,screenshot,state:safe(last)}));}
      }
      if(readyToPause&&matches(last)&&last.status==='paused'){report.auditionPausedAtUnixMs=Date.now();finished=true;break;}
      await sleep(150);
    }
    await sleep(600);const frozen=await state(),metadataAfter=(await call('get_track',{trackId:spec.incidents[0].id})).track;
    const checks={normalPlaybackUsesDownload:local.candidate===null&&local.quality?.sourceKind==='localAsset',
      chosenOnlineSource:finished&&matches(last),paused:finished&&frozen.status==='paused'&&Math.abs(frozen.position-last.position)<.15,
      savedMetadataUnchanged:Boolean(metadataBefore&&metadataAfter)&&JSON.stringify(metadataBefore)===JSON.stringify(metadataAfter),
      noDownloadsQueued:(await call('download_jobs',{offset:0,limit:1})).total===jobsBefore.total,
      queuedMetadataRetained:auditionQueueMetadataRetained(last.queue?.[0],metadataBefore)};
    report.passed=Object.values(checks).every(Boolean);report.cases.push({passed:report.passed,checks,local:safe(local),last:safe(last),frozen:safe(frozen),screenshot});
    await writeFile(`${profile}/ui-step.json`,JSON.stringify({phase:'completed',passed:report.passed,screenshot,checks}));
  } else if(mode==='live-analysis'){
    await call('play_tracks',{trackIds:[spec.ids[0]]});
    const estimate=async()=>{
      const deadline=performance.now()+23000;
      while(performance.now()<deadline){
        const snapshot=await state(),bpm=await call('get_live_bpm');report.samples.push({...safe(snapshot),bpm});
        if(bpm && Math.abs(bpm.bpm-120)<1 && bpm.trackId===snapshot.trackId && bpm.generation===snapshot.generation && bpm.seekSerial===snapshot.seekSerial){
          // The opt-in UI polls every six seconds. Do not fail a correct UI
          // merely because this independent API query won the analysis race.
          let layout=await call('inspect_playback_layout');const uiDeadline=performance.now()+9000;
          while(layout.liveBpmDiagnostics?.status!=='estimating'&&performance.now()<uiDeadline){await sleep(750);layout=await call('inspect_playback_layout');}
          const current=await state();
          if(current.generation!==snapshot.generation||current.seekSerial!==snapshot.seekSerial)throw Error('Analysis owner changed during UI observation');
          return {snapshot:safe(current),bpm,layout};
        }
        await sleep(1000);
      }
      throw Error('Native did not estimate the known 120 BPM source under the current owner');
    };
    const first=await estimate();await call('pause_playback');const pausedBpm=await call('get_live_bpm');
    await call('seek_playback',{position:24});const seekState=await state(),seekPausedBpm=await call('get_live_bpm');
    await call('resume_playback');const after=await estimate();
    await call('set_player_panel',{panel:'info'});await sleep(500);report.infoScreenshot=await captureUi('live-analysis-info');
    const checks={knownTempo:Math.abs(first.bpm.bpm-120)<1&&Math.abs(after.bpm.bpm-120)<1,
      pausedUnavailable:pausedBpm===null&&seekPausedBpm===null,seekOwner:after.bpm.seekSerial>first.bpm.seekSerial&&after.bpm.seekSerial===seekState.seekSerial,
      waveform:first.layout.waveformDiagnostics?.reasonCode==='waveform-ready'&&after.layout.waveformDiagnostics?.reasonCode==='waveform-ready'};
    report.runtimeChecksPassed=Object.values(checks).every(Boolean);
    report.uiStatusPassed=[first,after].every(item=>item.layout.liveBpmDiagnostics?.status==='estimating');
    report.passed=report.runtimeChecksPassed&&report.uiStatusPassed;
    report.cases.push({passed:report.passed,checks,first,after,seekState:safe(seekState),pausedBpm,seekPausedBpm});
    await call('pause_playback');
  } else if(mode==='waveform'){
    await call('play_tracks',{trackIds:[spec.ids[0]]});
    let initial,last,ready=false;
    for(let i=0;i<60;i++){
      const snapshot=await state();last=await call('inspect_playback_layout');
      report.samples.push({ms:i*100,...safe(snapshot),waveform:last.waveformDiagnostics});
      initial ??= last;
      if(snapshot.position>2 && last.waveformDiagnostics?.reasonCode==='waveform-ready'){ready=true;break;}
      await sleep(100);
    }
    await call('pause_playback');const paused=await state();await sleep(500);const frozen=await state();
    await call('seek_playback',{position:8});await call('resume_playback');await sleep(1100);
    const after=await state(),layout=await call('inspect_playback_layout');
    const w=layout.waveformDiagnostics;
    const checks={observedWaveform:ready,displayBinding:w?.displayBindingMatches===true&&w?.generationMatches===true&&w?.trackMatches===true,
      sampledBins:w?.populatedBins>0,paused:Math.abs(paused.position-frozen.position)<.15,
      seekOwner:after.seekSerial>paused.seekSerial&&after.position>8.5,analysisOff:w?.backgroundAnalysisEnabled===false};
    report.passed=Object.values(checks).every(Boolean);
    report.cases.push({passed:report.passed,checks,initial,last,afterLayout:layout,paused:safe(paused),after:safe(after)});
    await call('pause_playback');
  } else if(mode==='full-nipsey'){
    const ids=spec.incidents.map(track=>track.id),start=performance.now(),startUnixMs=Date.now();let receiptError=null;
    const receipt=call('play_tracks',{trackIds:ids}).catch(error=>{receiptError=error.message;});
    let entries=null,last=null,finished=false,monotonic=true,previous=null;
    const playing=[],maximum={},durations={},windows={};let identified=false,activeTrackId=null;
    while(performance.now()-start<900000){
      last=await state();const ms=performance.now()-start;report.samples.push({ms,...safe(last)});
      if(last.queueEntryIds?.length===ids.length)entries ??= [...last.queueEntryIds];
      if(last.status==='playing'){
        if(playing.at(-1)!==last.entryId){
          if(activeTrackId&&windows[activeTrackId])windows[activeTrackId].endedAtUnixMs=startUnixMs+ms;
          activeTrackId=last.trackId;playing.push(last.entryId);windows[last.trackId]={startedAtUnixMs:startUnixMs+ms};
        }
        maximum[last.trackId]=Math.max(maximum[last.trackId]??0,last.position);
        if(last.duration>0)durations[last.trackId]=last.duration;
        if(previous?.entryId===last.entryId&&previous.status==='playing'&&last.position+.2<previous.position)monotonic=false;
        if(last.trackId===ids[2]&&last.position>35&&!identified){
          identified=true;
          const metadataBefore=await call('get_track',{trackId:ids[2]});
          try{
            const result=await call('identify_playing_track',{captureId:crypto.randomUUID()},65000),after=await state();
            report.recognition={result,ownerStable:after.generation===last.generation&&after.entryId===last.entryId&&after.seekSerial===last.seekSerial,
              metadataUnchanged:JSON.stringify(metadataBefore)===JSON.stringify(await call('get_track',{trackId:ids[2]})),
              matches:result?.title?result.title.toLowerCase().includes('sacrifices')&&result.artist?.toLowerCase().includes('nipsey'):null};
          }catch(error){report.recognition={error:error.message,matches:null};}
        }
      }
      if(last.status==='ended'&&last.trackId===ids.at(-1)){
        maximum[last.trackId]=last.position;durations[last.trackId]=last.duration;
        if(windows[last.trackId])windows[last.trackId].endedAtUnixMs=Date.now();finished=true;break;
      }
      previous=safe(last);await sleep(250);
    }
    await receipt;
    const stable=entries&&JSON.stringify(entries)===JSON.stringify(last.queueEntryIds),once=entries&&JSON.stringify(playing)===JSON.stringify(entries);
    report.cases=spec.incidents.map(track=>({trackId:track.id,title:track.title,passed:finished&&(durations[track.id]??0)>0&&(maximum[track.id]??0)>=durations[track.id]*.95,
      maximumPosition:maximum[track.id]??0,decodedDuration:durations[track.id]??0,...windows[track.id]}));
    report.failureEvidence={lastSkippedTrackFailure:last.diagnostics?.lastSkippedTrackFailure??null,recentTrackFailures:last.diagnostics?.recentTrackFailures??[]};
    report.queue={finished,stable,once,monotonic,receiptError,playing,entries,last:safe(last)};
    report.passed=finished&&stable&&once&&monotonic&&!receiptError&&report.cases.every(item=>item.passed)&&report.recognition?.matches===true&&report.recognition?.ownerStable===true&&report.recognition?.metadataUnchanged===true
      &&report.failureEvidence.lastSkippedTrackFailure===null&&report.failureEvidence.recentTrackFailures.length===0;
    await call('pause_playback');
  } else if(mode==='full-incidents'){
    const ids=spec.incidents.map(track=>track.id),start=performance.now();let receiptError=null;
    const receipt=call('play_tracks',{trackIds:ids}).catch(error=>{receiptError=error.message;});
    let entries=null,last=null,finished=false,monotonic=true;
    const playing=[],maximum={};let previous=null;
    while(performance.now()-start<480000){
      last=await state();report.samples.push({ms:performance.now()-start,...safe(last)});
      if(last.queueEntryIds?.length===3)entries ??= [...last.queueEntryIds];
      if(last.status==='playing'){
        if(playing.at(-1)!==last.entryId)playing.push(last.entryId);
        maximum[last.trackId]=Math.max(maximum[last.trackId]??0,last.position);
        if(previous?.entryId===last.entryId && previous.status==='playing' && last.position+.2<previous.position)monotonic=false;
      }
      if(last.status==='ended' && last.trackId===ids[2]){maximum[ids[2]]=Math.max(maximum[ids[2]]??0,last.position);finished=true;break;}
      previous=safe(last);await sleep(250);
    }
    await receipt;
    const stable=entries && JSON.stringify(entries)===JSON.stringify(last.queueEntryIds);
    const once=entries && JSON.stringify(playing)===JSON.stringify(entries.slice(1));
    report.cases=spec.incidents.map((track,index)=>({trackId:track.id,title:track.title,passed:index>0&&finished&&(maximum[track.id]??0)>=track.durationSeconds*.95,
      outcome:index===0?'unqualified source skipped':finished?'natural completion':'incomplete',maximumPosition:maximum[track.id]??0,requestedDuration:track.durationSeconds}));
    report.passed=finished&&stable&&once&&monotonic&&report.cases.slice(1).every(item=>item.passed);
    report.queue={finished,stable,once,monotonic,receiptError,playing,entries,last:safe(last)};
    await call('pause_playback');
  } else if(mode==='mpris'){
    const bus=process.env.BURRATA_QUAL_PRIVATE_DBUS;
    if(!bus?.startsWith('unix:path=/tmp/dbus-'))throw Error('Verified private MPRIS bus required');
    const invoke=async(method,...args)=>(await promisify(execFile)('gdbus',['call','--address',bus,'--dest','org.mpris.MediaPlayer2.burrata','--object-path','/org/mpris/MediaPlayer2','--method',method,...args],{timeout:5000})).stdout;
    const player='org.mpris.MediaPlayer2.Player.';
    await call('play_tracks',{trackIds:spec.ids});await sleep(1200);const first=await state();
    const metadata=await invoke('org.freedesktop.DBus.Properties.Get','org.mpris.MediaPlayer2.Player','Metadata');
    await invoke(player+'Pause');const paused=await state();await sleep(700);const frozen=await state();
    await invoke(player+'Play');await sleep(700);const resumed=await state();
    await invoke(player+'Seek','2000000');await sleep(500);const sought=await state();
    await invoke('org.freedesktop.DBus.Properties.Set','org.mpris.MediaPlayer2.Player','Volume','<0.4>');const volume=await state();
    await invoke(player+'Next');await sleep(200);const next=await state();
    await invoke(player+'Previous');await sleep(200);const previous=await state();
    await invoke(player+'Stop');const stopped=await state();
    const checks={decodedDuration:metadata.includes('mpris:length')&&metadata.includes('12000000'),
      pause:paused.paused&&frozen.paused&&Math.abs(paused.position-frozen.position)<.15,
      resume:resumed.status==='playing'&&resumed.position-frozen.position>.4,
      seek:sought.seekSerial>resumed.seekSerial&&sought.position>resumed.position+1.5,
      volume:Math.abs(volume.volume-.4)<.00001,next:next.currentIndex===1&&next.trackId===spec.ids[1],
      previous:previous.currentIndex===0&&previous.trackId===spec.ids[0],stop:stopped.status==='idle'&&stopped.position===0,
      queueStable:JSON.stringify(first.queueEntryIds)===JSON.stringify(stopped.queueEntryIds)};
    report.passed=Object.values(checks).every(Boolean);
    report.cases.push({passed:report.passed,checks,metadata,first:safe(first),paused:safe(paused),frozen:safe(frozen),resumed:safe(resumed),sought:safe(sought),next:safe(next),previous:safe(previous),stopped:safe(stopped)});
  } else if(mode==='migration'){
    const before=await state();await sleep(600);const after=await state();
    report.passed=after.engine==='native' && after.status==='paused' && before.status==='paused'
      && after.currentIndex===1 && after.position===5 && before.position===after.position
      && after.repeat==='all' && after.shuffle===true && Math.abs(after.volume-.37)<.000001
      && JSON.stringify(after.queueEntryIds)===JSON.stringify(['legacy-first','legacy-second'])
      && after.queue?.length===2 && after.queue[0].id===after.queue[1].id;
    report.cases.push({passed:report.passed,before:safe(before),after:safe(after),volume:after.volume,repeat:after.repeat,shuffle:after.shuffle});
  } else if (mode === 'performance') {
    report.windows=[];
    let start=Date.now();await sleep(15000);report.windows.push({phase:'idle',startUnixMs:start,endUnixMs:Date.now()});
    await call('play_tracks',{trackIds:[spec.ids[0]]});
    let ready=false;
    for(let i=0;i<100;i++){const s=await state();if(s.status==='playing' && Number.isFinite(s.diagnostics?.firstAudioMs)){ready=true;break;}await sleep(100);}
    start=Date.now();const before=await state();await sleep(8000);const after=await state();
    report.windows.push({phase:'playback',startUnixMs:start,endUnixMs:Date.now()});
    report.passed=ready && after.status==='playing' && after.entryId===before.entryId && after.position-before.position>7;
    await call('pause_playback');start=Date.now();await sleep(15000);report.windows.push({phase:'paused',startUnixMs:start,endUnixMs:Date.now()});
    report.cases.push({passed:report.passed,before:safe(before),after:safe(after)});
  } else if (mode === 'radio') {
    const started=performance.now();let receiptError=null;
    const receipt=call('play_radio_station',{stationId:'main'}).catch(e=>{receiptError=e.message;});
    let anchor=null,last=null,stable=true,pausePassed=null;
    while(performance.now()-started<85000) {
      last=await state();report.samples.push({ms:performance.now()-started,...safe(last),radioNowPlaying:last.radioNowPlaying});
      if(last.status==='playing' && Number.isFinite(last.diagnostics?.firstAudioMs)) {
        anchor ??= {ms:performance.now(),position:last.position,entryId:last.entryId};
        stable &&= last.entryId===anchor.entryId;
        if(last.position-anchor.position>=60)break;
      }
      if(['failed','error','ended'].includes(last.status))break;
      await sleep(250);
    }
    await receipt;
    if(anchor && last.position-anchor.position>=60) {
      await call('pause_playback');const before=await state();await sleep(900);const after=await state();
      pausePassed=before.paused && after.paused && Math.abs(before.position-after.position)<.15;
      await call('resume_playback');await sleep(1500);const resumed=await state();
      pausePassed &&= resumed.status==='playing' && resumed.position-after.position>.5;
    }
    report.passed=!!anchor && stable && !receiptError && last.position-anchor.position>=60 && pausePassed===true;
    report.cases.push({passed:report.passed,receiptError,pausePassed,last:safe(last),metadata:last.radioNowPlaying,radioHistory:last.radioHistory});
    await call('pause_playback');
  } else if (mode === 'incidents') {
    for (const track of spec.incidents) {
      const started=performance.now(), startedAtUnixMs=Date.now();let receiptError=null;
      const receipt=call('play_tracks',{trackIds:[track.id]}).catch(e=>{receiptError=e.message;});
      let anchor=null,last=null,passed=false;
      while(performance.now()-started<45000) {
        last=await state();
        report.samples.push({ms:performance.now()-started,caseId:track.id,...safe(last)});
        if(last.trackId===track.id && last.status==='playing' && Number.isFinite(last.diagnostics?.firstAudioMs)) {
          anchor ??= last.position;
          if(last.position-anchor>=10){passed=true;break;}
        }
        if(last.trackId===track.id && ['failed','error','ended'].includes(last.status))break;
        await sleep(150);
      }
      await receipt;
      const item={trackId:track.id,title:track.title,passed:passed&&!receiptError,startedAtUnixMs,elapsedMs:performance.now()-started,receiptError,state:safe(last)};
      report.cases.push(item);console.log(JSON.stringify(item));
      await call('pause_playback');
      // Export only the app's deliberately sanitized source report.
      const trace=await call('get_playback_diagnostics_report').catch(()=>null);
      if(trace)await writeFile(`${profile}/incident-source-${report.cases.length}.json`,JSON.stringify(trace,null,2));
    }
    report.passed=report.cases.length===spec.incidents.length && report.cases.every(c=>c.passed);
  } else {
    const ids = mode==='aac' ? [spec.aacId] : mode==='seek'||mode.startsWith('stereo-') ? [spec.ids[0]] : spec.ids;
    if(mode.startsWith('normalization-')){
      report.priming=[];
      for(const id of ids){await call('play_tracks',{trackIds:[id]});await sleep(1800);report.priming.push(safe(await state()));await call('pause_playback');await sleep(250);}
      report.mainPlaybackStartedAtUnixMs=Date.now();
    }
    const start=performance.now();let receiptError=null,seeked=false,paused=false,pausePassed=null,finished=false;
    const receipt=call('play_tracks',{trackIds:ids}).catch(e=>{receiptError=e.message;});
    let entries=null;
    while(performance.now()-start<60000) {
      const s=await state();report.samples.push({ms:performance.now()-start,...safe(s)});
      if(s.trackId===ids[0] && s.queueEntryIds?.length===ids.length)entries ??= [...s.queueEntryIds];
      if((mode==='seek'||mode==='stereo-seek-opus') && !seeked && s.trackId===ids[0] && s.status==='playing' && s.position>(mode==='seek'?1:1.5)) {
        const before=performance.now(),target=mode==='seek'?10.5:6.5;await call('seek_playback',{position:target});
        report.seek={requested:target,ms:before-start};seeked=true;
      }
      if(mode==='crossfade-pause' && !paused && s.transition?.progress>.2 && s.transition?.progress<.8) {
        await call('pause_playback');const before=await state();await sleep(800);const after=await state();
        pausePassed=before.paused && after.paused && before.entryId===after.entryId
          && Math.abs(before.position-after.position)<.15
          && Math.abs((before.transition?.progress??0)-(after.transition?.progress??0))<.1;
        report.pause={before:safe(before),after:safe(after),passed:pausePassed};
        await call('resume_playback');paused=true;
      }
      if(['failed','error'].includes(s.status))break;
      if(s.trackId===ids.at(-1) && s.status==='ended'){finished=true;break;}
      await sleep(80);
    }
    await receipt;await sleep(350);
    const last=await state();
    const observed=report.samples.filter(s=>s.status==='playing').map(s=>s.entryId).filter((id,i,a)=>i===0||id!==a[i-1]);
    const stable=entries && JSON.stringify(entries)===JSON.stringify(last.queueEntryIds);
    const once=entries && JSON.stringify(observed)===JSON.stringify(entries);
    const decodedDuration=Number.isFinite(last.duration) && Math.abs(last.duration-12)<.3;
    report.passed=finished && stable && once && decodedDuration && !receiptError
      && (mode!=='seek'||seeked && report.samples.at(-1).ms-report.seek.ms>1000 && report.samples.at(-1).ms-report.seek.ms<3000) && (mode!=='stereo-seek-opus'||seeked&&report.samples.at(-1).ms-report.seek.ms>4500&&report.samples.at(-1).ms-report.seek.ms<7500) && (mode!=='crossfade-pause'||pausePassed===true);
    report.cases.push({finished,stable,once,decodedDuration,observed,entries,receiptError,last:safe(last)});
    await call('pause_playback');
  }
} catch(e) {report.passed=false;report.error=e.message;}
finally {
  await writeFile(`${profile}/runtime-result.json`,JSON.stringify(report,null,2));
  await client.close().catch(()=>{});
}
console.log(JSON.stringify({mode,passed:report.passed,error:report.error,cases:report.cases}));
if(!report.passed)process.exitCode=1;
