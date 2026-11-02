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
