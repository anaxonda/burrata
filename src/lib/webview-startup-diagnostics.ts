export type WebviewStartupPhase = 'attach' | 'resume_seek' | 'play';
export function captureWebviewStartupFailure(audio: HTMLAudioElement, phase: WebviewStartupPhase, mimeType: string | undefined, streamKind: string | undefined, resumeRequested: boolean, elapsedMs: number) {
  const rawMime = (mimeType ?? '').split(';',1)[0]?.trim().toLowerCase();
  const mimeFamily = ['audio/mpeg','audio/mp4','audio/webm','audio/ogg','audio/flac','audio/wav','application/vnd.apple.mpegurl','application/x-mpegurl'].includes(rawMime) ? rawMime : rawMime ? 'other' : 'unknown';
  let canPlayType: 'probably'|'maybe'|'no'|'unknown' = 'unknown';
  if(mimeFamily!=='other'&&mimeFamily!=='unknown') {
    try { const value=audio.canPlayType(mimeFamily);canPlayType=value==='probably'||value==='maybe'?value:'no'; }catch{}
  }
  const bounded=(value:number,maximum:number)=>Number.isInteger(value)&&value>=0&&value<=maximum?value:null;
  return {phase,mimeFamily,canPlayType,streamKind:['direct','native-hls','mse-hls'].includes(streamKind??'')?streamKind:'unknown',resumeRequested,
    readyState:bounded(audio.readyState,4),networkState:bounded(audio.networkState,3),mediaErrorCode:audio.error?bounded(audio.error.code,4):null,
    elapsedMs:Number.isFinite(elapsedMs)?Math.min(60000,Math.max(0,Math.round(elapsedMs))):null};
}
export type WebviewStartupFailure = ReturnType<typeof captureWebviewStartupFailure>;
