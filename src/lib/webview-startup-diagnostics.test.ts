import {expect,it} from 'vitest';
import {captureWebviewStartupFailure} from './webview-startup-diagnostics';
it('returns only finite startup state without hostile MIME values or media messages',()=>{
 const audio={readyState:0,networkState:2,error:{code:3,message:'https://private.invalid/token'},canPlayType:()=>'',currentTime:Infinity,duration:Infinity,seeking:true,buffered:{length:1,start:()=>0,end:()=>Infinity},seekable:{length:0}} as unknown as HTMLAudioElement;
 const result=captureWebviewStartupFailure(audio,'play','private-cookie-value','private-stream',true,Infinity,349,'https://private.invalid/token');
 expect(result).toEqual({phase:'play',mimeFamily:'other',canPlayType:'unknown',streamKind:'unknown',sourceTransport:'unknown',resumeRequested:true,readyState:0,networkState:2,mediaErrorCode:3,currentTime:null,duration:null,seeking:true,requestedSeekPosition:349,bufferedRanges:[],seekableRanges:[],elapsedMs:null});
 expect(JSON.stringify(result)).not.toMatch(/private|token|cookie/);
});
it('records known MIME capability and bounded numbers only',()=>{
 const audio={readyState:4,networkState:1,error:null,canPlayType:()=> 'probably'} as unknown as HTMLAudioElement;
 expect(captureWebviewStartupFailure(audio,'attach','audio/mpeg; codecs=mp3','direct',false,90000,349.437,'loopback-relay')).toMatchObject({mimeFamily:'audio/mpeg',canPlayType:'probably',streamKind:'direct',sourceTransport:'loopback-relay',elapsedMs:60000,requestedSeekPosition:349.437});
});
it('captures bounded media timeline and range state without leaking source strings',()=>{
 const audio={readyState:2,networkState:2,error:null,canPlayType:()=> 'maybe',currentTime:349.4,duration:7202.1,seeking:true,buffered:{length:2,start:(i:number)=>[340,7000][i],end:(i:number)=>[360,7202][i]},seekable:{length:1,start:()=>0,end:()=>7202}} as unknown as HTMLAudioElement;
 const result=captureWebviewStartupFailure(audio,'resume_seek','audio/mp4','direct',true,18785,349.437,'loopback-relay');
 expect(result).toMatchObject({phase:'resume_seek',currentTime:349.4,duration:7202.1,seeking:true,requestedSeekPosition:349.437,bufferedRanges:[[340,360],[7000,7202]],seekableRanges:[[0,7202]],sourceTransport:'loopback-relay'});
 expect(JSON.stringify(result)).not.toMatch(/https?:|token|private/);
});

import {vi} from 'vitest';
import {waitForStartupClock, startupStageTimings} from './webview-startup-diagnostics';
it('does not accept play request until the decoder advances and cancels obsolete attachments', async()=>{
 vi.useFakeTimers();
 try {
  const audio={currentTime:0, paused:false, seeking:false, readyState:2, error:null};
  let current=true;
  let done=false;
  const pending=waitForStartupClock(audio,0,()=>current,performance.now()+1000).then(()=>{done=true;});
  await vi.advanceTimersByTimeAsync(100); expect(done).toBe(false);
  audio.currentTime=.3;
  await vi.advanceTimersByTimeAsync(25); await pending; expect(done).toBe(true);
  const cancelled=waitForStartupClock(audio,.3,()=>current,performance.now()+1000);
  const rejection=expect(cancelled).rejects.toThrow('cancelled');
  current=false; await vi.advanceTimersByTimeAsync(25); await rejection;
 } finally {vi.useRealTimers();}
});
it('fails a stalled clock and records stage timings without source data', async()=>{
 vi.useFakeTimers();
 try {
  const stages=startupStageTimings();stages.mark('attach');
  await vi.advanceTimersByTimeAsync(50);stages.mark('play');
  expect(stages.snapshot()).toEqual({attach:0,play:50});
  const pending=waitForStartupClock({currentTime:0,paused:false,seeking:false,readyState:2,error:null},0,()=>true,performance.now()+50);
  const rejection=expect(pending).rejects.toThrow('deadline');
  await vi.advanceTimersByTimeAsync(50);await rejection;
 } finally {vi.useRealTimers();}
});

it('accepts a short final resume only after that source reaches its end', async()=>{
 const audio={currentTime:10, duration:10, ended:true, paused:true, seeking:false, readyState:2, error:null};
 await expect(waitForStartupClock(audio,9.9,()=>true,performance.now()+100)).resolves.toBeUndefined();
 await expect(waitForStartupClock(audio,10,()=>true,performance.now())).rejects.toThrow('deadline');
});
