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
