export function startupObservation(state, targetTrackId) {
  const firstAudioMs=state?.diagnostics?.firstAudioMs;
  const generation=state?.generation;
  if(state?.trackId!==targetTrackId||state?.status!=='playing'
      ||!Number.isSafeInteger(generation)||generation<=0
      ||!Number.isFinite(firstAudioMs)||firstAudioMs<0
      ||!Number.isFinite(state?.position)||state.position<3)return null;
  return {trackId:targetTrackId,generation,firstAudioMs,position:state.position,status:state.status};
}

export function startupPauseGate(observation, pausedState, targetTrackId) {
  return !!observation&&observation.trackId===targetTrackId&&observation.status==='playing'
    &&Number.isSafeInteger(observation.generation)&&observation.generation>0
    &&Number.isFinite(observation.firstAudioMs)&&observation.firstAudioMs>=0
    &&Number.isFinite(observation.position)&&observation.position>=3
    &&pausedState?.trackId===targetTrackId&&pausedState?.generation===observation.generation
    &&pausedState?.status==='paused'&&pausedState?.paused===true
    &&Number.isFinite(pausedState?.position)&&pausedState.position>=3;
}

const STARTUP_OPERATION_PHASES=new Set(['ipc_received','source_ready','first_audio']);
const STARTUP_OPERATION_KINDS=new Set(['startup','transition','pause','resume','seek','volume']);
const STARTUP_PLAYBACK_STATUSES=new Set(['idle','resolving','playing','paused','buffering','ended','failed','stopped']);
const STARTUP_DECODER_KINDS=new Set(['rodio','ffmpeg','experimental_opus','experimental-opus','other']);
const STARTUP_DIAGNOSTIC_PHASES=new Set(['decoder_start','decoder_end']);
const STARTUP_DIAGNOSTIC_PROVIDERS=new Set(['youtube','soundcloud','monochrome','bandcamp','musicbrainz','other']);
const STARTUP_NATIVE_MOMENTS=new Set(['snapshot','progress','clean_eof','premature_eof','output_ended']);
const MAX_STARTUP_DIAGNOSTIC_MS=86_400_000;

function boundedDiagnosticInteger(value, maximum=MAX_STARTUP_DIAGNOSTIC_MS) {
  return Number.isSafeInteger(value)&&value>=0&&value<=maximum?value:null;
}

function safeStartupOperationTrace(value) {
  if(!Array.isArray(value))return [];
  return value.slice(0,3).flatMap(row=>{
    if(!row||typeof row!=='object'||!STARTUP_OPERATION_PHASES.has(row.phase))return [];
    const atMs=boundedDiagnosticInteger(row.atMs);
    return atMs===null?[]:[{phase:row.phase,atMs}];
  });
}

function safeStartupSnapshot(snapshot,targetGeneration) {
  if(!snapshot||typeof snapshot!=='object')return null;
  const generation=boundedDiagnosticInteger(snapshot.generation,9_007_199_254_740_991);
  if(generation===null||generation!==targetGeneration)return null;
  const diagnostics=snapshot.diagnostics&&typeof snapshot.diagnostics==='object'?snapshot.diagnostics:{};
  const operationTrace=safeStartupOperationTrace(diagnostics.operationTrace);
  const operationFirstAudioMs=operationTrace.find(row=>row.phase==='first_audio')?.atMs??null;
  const result={
    generation,
    status:STARTUP_PLAYBACK_STATUSES.has(snapshot.status)?snapshot.status:null,
    operationKind:STARTUP_OPERATION_KINDS.has(diagnostics.operationKind)?diagnostics.operationKind:null,
    sourceReadyMs:boundedDiagnosticInteger(diagnostics.sourceReadyMs),
    firstAudioMs:boundedDiagnosticInteger(diagnostics.firstAudioMs),
    operationFirstAudioMs,
    operationTrace,
  };
  const rawDecoder=diagnostics.decoder&&typeof diagnostics.decoder==='object'?diagnostics.decoder:{};
  const decoder={};
  for(const key of ['readerSetupMs','decoderSetupMs','firstDecodedBlockMs']){
    const number=boundedDiagnosticInteger(rawDecoder[key]);
    if(number!==null)decoder[key]=number;
  }
  for(const key of ['bytesRead','bytesReceived','inputContentLength']){
    const number=boundedDiagnosticInteger(rawDecoder[key],1_000_000_000_000);
    if(number!==null)decoder[key]=number;
  }
  for(const [key,maximum] of [['sampleRate',1_000_000],['channels',64]]){
    const number=boundedDiagnosticInteger(rawDecoder[key],maximum);
    if(number!==null)decoder[key]=number;
  }
  for(const key of ['decoderReused','readerReused']){
    if(typeof rawDecoder[key]==='boolean')decoder[key]=rawDecoder[key];
  }
  if(STARTUP_DECODER_KINDS.has(rawDecoder.decoderKind))decoder.decoderKind=rawDecoder.decoderKind;
  if(Object.keys(decoder).length)result.decoder=decoder;
  return result;
}

export function safeStartupMarkerMetrics(snapshot,targetGeneration) {
  const safe=safeStartupSnapshot(snapshot,boundedDiagnosticInteger(targetGeneration,9_007_199_254_740_991));
  if(!safe)return null;
  return {generation:safe.generation,firstAudioMs:safe.firstAudioMs,
    sourceReadyMs:safe.sourceReadyMs,operationFirstAudioMs:safe.operationFirstAudioMs};
}

function safeStartupDecoderEvents(value,targetGeneration) {
  if(!Array.isArray(value))return [];
  return value.slice(-64).flatMap(event=>{
    if(!event||typeof event!=='object'||!STARTUP_DIAGNOSTIC_PHASES.has(event.phase))return [];
    const elapsedMs=boundedDiagnosticInteger(event.elapsedMs);
    const details=event.details&&typeof event.details==='object'?event.details:{};
    const generation=boundedDiagnosticInteger(details.generation,9_007_199_254_740_991);
    if(elapsedMs===null||generation!==targetGeneration)return [];
    const safe={phase:event.phase,elapsedMs,generation,
      provider:STARTUP_DIAGNOSTIC_PROVIDERS.has(event.provider)?event.provider:null};
    const atUnixMs=boundedDiagnosticInteger(event.atUnixMs,9_000_000_000_000);
    if(atUnixMs!==null&&atUnixMs>=1_000_000_000_000)safe.atUnixMs=atUnixMs;
    const durationMs=boundedDiagnosticInteger(details.durationMs);
    if(durationMs!==null)safe.durationMs=durationMs;
    if(details.outcome==='success'||details.outcome==='failed')safe.outcome=details.outcome;
    if(details.category==='decoder')safe.category=details.category;
    return [safe];
  });
}

function safeStartupNativePlayback(value,targetGeneration) {
  if(!Array.isArray(value))return [];
  return value.slice(-64).flatMap(event=>{
    if(!event||typeof event!=='object'||!STARTUP_NATIVE_MOMENTS.has(event.moment))return [];
    const elapsedMs=boundedDiagnosticInteger(event.elapsedMs);
    const playback=event.playback&&typeof event.playback==='object'?event.playback:{};
    const generation=boundedDiagnosticInteger(playback.generation,9_007_199_254_740_991);
    if(elapsedMs===null||generation!==targetGeneration)return [];
    const result={moment:event.moment,elapsedMs,generation,
      status:STARTUP_PLAYBACK_STATUSES.has(playback.status)?playback.status:null};
    const atUnixMs=boundedDiagnosticInteger(event.atUnixMs,9_000_000_000_000);
    if(atUnixMs!==null&&atUnixMs>=1_000_000_000_000)result.atUnixMs=atUnixMs;
    for(const key of ['firstAudioMs','sourceReadyMs','seekFirstAudioMs','seekOperationFirstAudioMs','clickToIpcMs']){
      const number=boundedDiagnosticInteger(playback[key]);
      if(number!==null)result[key]=number;
    }
    if(STARTUP_DECODER_KINDS.has(playback.decoderKind))result.decoderKind=playback.decoderKind;
    if(playback.decoderEvidence==='lastStartedSnapshot')result.decoderEvidence=playback.decoderEvidence;
    if(playback.decoderGenerationBinding==='notRecorded')result.decoderGenerationBinding=playback.decoderGenerationBinding;
    return [result];
  });
}

export function safeStartupDiagnosticExport({playingSnapshot,pausedSnapshot,diagnosticsReport,targetGeneration}) {
  const generation=boundedDiagnosticInteger(targetGeneration,9_007_199_254_740_991);
  const status=diagnosticsReport?.status&&typeof diagnosticsReport.status==='object'?diagnosticsReport.status:{};
  return {
    scope:'opt-in playback diagnostics captured after startup timing and pause',
    reportAvailable:Boolean(diagnosticsReport&&typeof diagnosticsReport==='object'
      &&diagnosticsReport.status&&typeof diagnosticsReport.status==='object'),
    targetGeneration:generation,
    playingSnapshot:safeStartupSnapshot(playingSnapshot,generation),
    pausedSnapshot:safeStartupSnapshot(pausedSnapshot,generation),
    diagnosticStatus:{enabled:status.enabled===true,
      eventCount:boundedDiagnosticInteger(status.eventCount,1_000_000),
      nativeEventCount:boundedDiagnosticInteger(status.nativeEventCount,1_000_000),
      droppedEvents:boundedDiagnosticInteger(status.droppedEvents,1_000_000),
      droppedNativeEvents:boundedDiagnosticInteger(status.droppedNativeEvents,1_000_000)},
    decoderEvents:safeStartupDecoderEvents(diagnosticsReport?.events,generation),
    nativePlaybackEvents:safeStartupNativePlayback(diagnosticsReport?.nativePlayback,generation),
  };
}

export function parseIdleIpcCounter(text) {
  let value;
  try { value=JSON.parse(text); } catch { return null; }
  if(!value||typeof value!=='object'||Array.isArray(value))return null;
  const keys=Object.keys(value).sort();
  if(keys.length!==2||keys[0]!=='count'||keys[1]!=='registered'
      ||!Number.isSafeInteger(value.registered)||value.registered<0
      ||!Number.isSafeInteger(value.count)||value.count<0)return null;
  return {registered:value.registered,count:value.count};
}

export function safeStoreMutexMetrics(status) {
  const metrics=status?.diagnostics?.storeConnectionWait;
  if(!metrics || metrics.scope!=='global_store_connection_mutex')return null;
  const number=value=>Number.isSafeInteger(value)&&value>=0?value:null;
  const operations={};
  const hasOperations=Boolean(metrics.operations && typeof metrics.operations==='object' && !Array.isArray(metrics.operations));
  if(hasOperations) {
    for(const [name,value] of Object.entries(metrics.operations)) {
      if(!/^[A-Za-z0-9_.:-]{1,100}$/.test(name)||!value||typeof value!=='object')continue;
      operations[name]={lockCount:number(value.lockCount),totalWaitNs:number(value.totalWaitNs),maxWaitNs:number(value.maxWaitNs),
        totalHoldNs:number(value.totalHoldNs),maxHoldNs:number(value.maxHoldNs)};
    }
  }
  return {scope:metrics.scope,lockCount:number(metrics.lockCount),totalWaitNs:number(metrics.totalWaitNs),
    maxWaitNs:number(metrics.maxWaitNs),operations:hasOperations?operations:null,
    holdMetricsAvailable:hasOperations&&Object.values(operations).some(value=>value.totalHoldNs!==null||value.maxHoldNs!==null),
    sqliteEngineBusyWait:'not_measured'};
}

export function safePlaybackFailureSummary(state, targetTrackId) {
  const failures=state?.diagnostics?.recentTrackFailures;
  if(!Array.isArray(failures))return null;
  const categories=['no_audio_results','recording_identity_unverified','no_playable_match','source_identity_mismatch',
    'unsupported_audio_stream','failed_with_reason'];
  const failure=[...failures].reverse().find(item=>item?.trackId===targetTrackId);
  if(!failure)return null;
  return {category:categories.includes(failure.category)?failure.category:null,
    outcome:['skipped','queue_exhausted'].includes(failure.outcome)?failure.outcome:null,
    provider:['youtube','soundcloud','monochrome'].includes(failure.provider)?failure.provider:null};
}

export function reportedErrorEvidence(receiptError, state, targetTrackId) {
  return {receiptFailed:Boolean(receiptError),failureEvidence:safePlaybackFailureSummary(state,targetTrackId)};
}

export function idleIpcBucketsGate(initial, warmups, measured, actualElapsedMs) {
  if(!initial||!Array.isArray(warmups)||warmups.length!==3
      ||!Array.isArray(measured)||measured.length!==20
      ||!Number.isSafeInteger(initial.registered)||initial.registered<1
      ||!Number.isSafeInteger(initial.count)||initial.count<0)return false;
  let previous=initial;
  for(const bucket of [...warmups,...measured]){
    if(!Number.isSafeInteger(bucket?.registered)||bucket.registered!==initial.registered
        ||!Number.isSafeInteger(bucket?.count)||bucket.count<previous.count
        ||!Number.isFinite(bucket?.windowMs)||bucket.windowMs<900)return false;
    previous=bucket;
  }
  const measuredElapsedMs=measured.reduce((sum,bucket)=>sum+bucket.windowMs,0);
  return Number.isFinite(actualElapsedMs)&&actualElapsedMs>=20700&&measuredElapsedMs>=18000;
}
