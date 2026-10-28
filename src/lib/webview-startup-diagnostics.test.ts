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
