export type WebviewStartupPhase = 'attach' | 'resume_seek' | 'play' | 'clock_progress';
export function captureWebviewStartupFailure(audio: HTMLAudioElement, phase: WebviewStartupPhase, mimeType: string | undefined, streamKind: string | undefined, resumeRequested: boolean, elapsedMs: number, requestedSeekPosition: number, sourceTransport?: string, stageTimings?: StartupStageTimings) {
  const rawMime = (mimeType ?? '').split(';',1)[0]?.trim().toLowerCase();
  const mimeFamily = ['audio/mpeg','audio/mp4','audio/webm','audio/ogg','audio/flac','audio/wav','application/vnd.apple.mpegurl','application/x-mpegurl'].includes(rawMime) ? rawMime : rawMime ? 'other' : 'unknown';
  let canPlayType: 'probably'|'maybe'|'no'|'unknown' = 'unknown';
  if(mimeFamily!=='other'&&mimeFamily!=='unknown') {
    try { const value=audio.canPlayType(mimeFamily);canPlayType=value==='probably'||value==='maybe'?value:'no'; }catch{}
  }
  const bounded=(value:number,maximum:number)=>Number.isInteger(value)&&value>=0&&value<=maximum?value:null;
  const boundedSeconds=(value:number)=>Number.isFinite(value)&&value>=0&&value<=86400?Math.round(value*1000)/1000:null;
  const ranges=(range:TimeRanges)=>{
    const result:Array<[number,number]>=[];
    try { for(let index=0;index<Math.min(range.length,8);index++){const start=boundedSeconds(range.start(index)),end=boundedSeconds(range.end(index));if(start!==null&&end!==null&&end>=start)result.push([start,end]);} } catch {}
    return result;
  };
  const safeStages = stageTimings ? Object.fromEntries((['attach','resume_seek','play','clock_progress'] as const)
    .filter(key => typeof stageTimings[key] === 'number' && Number.isFinite(stageTimings[key]))
    .map(key => [key, Math.min(60000, Math.max(0, Math.round(stageTimings[key]!)))])) : null;
  return {...(safeStages ? {stageTimings: safeStages} : {}),phase,mimeFamily,canPlayType,streamKind:['direct','native-hls','mse-hls'].includes(streamKind??'')?streamKind:'unknown',
    sourceTransport:['direct','loopback-relay','loopback-transcode'].includes(sourceTransport??'')?sourceTransport:'unknown',resumeRequested,
    readyState:bounded(audio.readyState,4),networkState:bounded(audio.networkState,3),mediaErrorCode:audio.error?bounded(audio.error.code,4):null,
    currentTime:boundedSeconds(audio.currentTime),duration:Number.isFinite(audio.duration)?boundedSeconds(audio.duration):null,seeking:audio.seeking,
    requestedSeekPosition:boundedSeconds(requestedSeekPosition),bufferedRanges:ranges(audio.buffered),seekableRanges:ranges(audio.seekable),
    elapsedMs:Number.isFinite(elapsedMs)?Math.min(60000,Math.max(0,Math.round(elapsedMs))):null};
}
export type WebviewStartupFailure = ReturnType<typeof captureWebviewStartupFailure>;

/** A fulfilled play() can precede decoding. Require this attachment's clock. */
export async function waitForStartupClock(
  audio: Pick<HTMLAudioElement, 'currentTime' | 'paused' | 'seeking' | 'readyState' | 'error'> & Partial<Pick<HTMLAudioElement, 'ended' | 'duration'>>,
  startPosition: number,
  stillCurrent: () => boolean,
  deadline: number,
): Promise<void> {
  while (stillCurrent()) {
    if (audio.error) throw new Error('The audio decoder could not start playback.');
    if (!audio.paused && !audio.seeking && audio.readyState >= 2 &&
        Number.isFinite(audio.currentTime) && audio.currentTime - startPosition >= .25) return;
    // A resume in the final fraction of a second can finish before .25s.
    if (audio.ended && !audio.seeking && audio.readyState >= 2 && Number.isFinite(audio.duration) &&
        Number.isFinite(audio.currentTime) && audio.currentTime > startPosition &&
        Math.abs(audio.currentTime - audio.duration!) < .05) return;
    if (performance.now() >= deadline) throw new Error('Audio startup deadline expired before audio progressed.');
    await new Promise<void>(resolve => setTimeout(resolve, 25));
  }
  throw new Error('Audio startup was cancelled.');
}

export type StartupStageTimings = Partial<Record<WebviewStartupPhase, number>>;
export function startupStageTimings(): { mark: (phase: WebviewStartupPhase) => void; snapshot: () => StartupStageTimings } {
  const started = performance.now();
  const stages: StartupStageTimings = {};
  return {
    mark: phase => { stages[phase] = Math.max(0, Math.round(performance.now() - started)); },
    snapshot: () => ({ ...stages }),
  };
}
