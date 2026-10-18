import {expect,it} from 'vitest';
import {captureWebviewStartupFailure} from './webview-startup-diagnostics';
it('returns only finite startup state without hostile MIME values or media messages',()=>{
 const audio={readyState:0,networkState:2,error:{code:3,message:'https://private.invalid/token'},canPlayType:()=>''} as unknown as HTMLAudioElement;
 const result=captureWebviewStartupFailure(audio,'play','private-cookie-value','private-stream',true,Infinity);
 expect(result).toEqual({phase:'play',mimeFamily:'other',canPlayType:'unknown',streamKind:'unknown',resumeRequested:true,readyState:0,networkState:2,mediaErrorCode:3,elapsedMs:null});
 expect(JSON.stringify(result)).not.toMatch(/private|token|cookie/);
});
it('records known MIME capability and bounded numbers only',()=>{
 const audio={readyState:4,networkState:1,error:null,canPlayType:()=> 'probably'} as unknown as HTMLAudioElement;
 expect(captureWebviewStartupFailure(audio,'attach','audio/mpeg; codecs=mp3','direct',false,90000)).toMatchObject({mimeFamily:'audio/mpeg',canPlayType:'probably',streamKind:'direct',elapsedMs:60000});
});
