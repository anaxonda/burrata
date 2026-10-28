import { validateLyricsTranslation, type LyricsSourceLine, type LyricsTranslationResult } from "./lyrics-translation";
export type TranslationProgress = (result: LyricsTranslationResult) => void;
export async function translateLyricsInChunks(sourceHash: string, lines: LyricsSourceLine[], request: (start:number,end:number)=>Promise<LyricsTranslationResult>, onProgress?:TranslationProgress, signal?:AbortSignal):Promise<LyricsTranslationResult> {
  if (!lines.length) throw new Error("No lyric lines to translate");
  const batches:{start:number;end:number}[]=[];
  for(let start=0;start<lines.length;){const end=Math.min(lines.length,start+(start===0?4:12));batches.push({start,end});start=end;}
  let next=0, stopped=false, metadata:LyricsTranslationResult|undefined;
  const complete=new Map<string,{id:string;text:string}>();
  const check=()=>{if(signal?.aborted)throw new DOMException("Translation cancelled","AbortError");};
  const snapshot=():LyricsTranslationResult=>({...metadata!,sourceHash,lines:lines.flatMap(line=>complete.has(line.id)?[complete.get(line.id)!]:[])});
  async function worker(){while(!stopped && next<batches.length){check();const {start,end}=batches[next++];try{
    const result=await request(start,end);check();if(stopped)return;
    if(!validateLyricsTranslation(sourceHash,lines.slice(start,end),result))throw new Error("A translation batch did not match the lyric source");
    if(metadata && result.targetLanguage.toLowerCase()!==metadata.targetLanguage.toLowerCase())throw new Error("Translation language changed between batches");
    metadata=result;for(const line of result.lines)complete.set(line.id,line);onProgress?.(snapshot());
  }catch(error){stopped=true;throw error;}}}
  await Promise.all(Array.from({length:Math.min(2,batches.length)},()=>worker()));check();
  const result=snapshot();if(!validateLyricsTranslation(sourceHash,lines,result))throw new Error("Translation is incomplete");return result;
}
