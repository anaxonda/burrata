import {expect,it,vi} from "vitest";
import {translateLyricsInChunks} from "./lyrics-translation-progress";
import {validateLyricsTranslationProgress} from "./lyrics-translation";
const lines=Array.from({length:24},(_,i)=>({id:`line-${i}`,timeSeconds:i,text:`Synthetic ${i}`}));
const result=(start:number,end:number)=>({sourceHash:"source",targetLanguage:"fr",lines:lines.slice(start,end).map(line=>({id:line.id,text:`French ${line.id}`}))});
it("publishes the first batch before completion, bounds concurrency, and restores source order",async()=>{
 const waiting=new Map<number,()=>void>();let active=0,peak=0;
 const request=vi.fn((start:number,end:number)=>{active++;peak=Math.max(peak,active);return new Promise<ReturnType<typeof result>>(resolve=>waiting.set(start,()=>{active--;resolve(result(start,end));}));});
 const progress=vi.fn();const work=translateLyricsInChunks("source",lines,request,progress);
 expect(request).toHaveBeenCalledTimes(2);waiting.get(4)!();await Promise.resolve();await Promise.resolve();
 expect(progress).toHaveBeenCalledTimes(1);expect(progress.mock.calls[0][0].lines[0].id).toBe("line-4");
 waiting.get(16)!();await Promise.resolve();await Promise.resolve();waiting.get(0)!();
 const complete=await work;expect(peak).toBe(2);expect(complete.lines.map(line=>line.id)).toEqual(lines.map(line=>line.id));
});
it("does not start another batch after cancellation",async()=>{
 const controller=new AbortController();const progress=vi.fn();let finish!:()=>void;
 const request=vi.fn((start:number,end:number)=>new Promise<ReturnType<typeof result>>(resolve=>{if(start===0)finish=()=>resolve(result(start,end));else resolve(result(start,end));}));
 const work=translateLyricsInChunks("source",lines,request,progress,controller.signal);controller.abort();finish();
 await expect(work).rejects.toMatchObject({name:"AbortError"});expect(request).toHaveBeenCalledTimes(2);expect(progress).not.toHaveBeenCalled();
});
it("rejects unknown, duplicated, reordered, or empty translated lines in progress",()=>{
 expect(validateLyricsTranslationProgress("source",lines,result(4,8))).not.toBeNull();
 expect(validateLyricsTranslationProgress("source",lines,{...result(0,2),lines:[{id:"line-1",text:"A"},{id:"line-0",text:"B"}]})).toBeNull();
 expect(validateLyricsTranslationProgress("source",lines,{...result(0,2),lines:[{id:"unknown",text:"A"}]})).toBeNull();
});
