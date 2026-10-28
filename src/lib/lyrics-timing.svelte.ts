const STORAGE_KEY="burrata.lyrics-timing.v1";
type Entry={seconds:number;updated:number};
function readSaved(storage:Pick<Storage,"getItem">|null=typeof window==="undefined"?null:window.localStorage):Record<string,Entry>{
  try{
    const raw=JSON.parse(storage?.getItem(STORAGE_KEY)??"{}");
    return Object.fromEntries(Object.entries(raw).filter(([key,value])=>key.length<=2048&&value&&typeof value==="object"&&Number.isFinite((value as Entry).seconds)&&Math.abs((value as Entry).seconds)<=30&&Number.isFinite((value as Entry).updated))) as Record<string,Entry>;
  }catch{return {};}
}
let entries=$state<Record<string,Entry>>(readSaved());
export const lyricsTiming={
  get(key:string){return entries[key]?.seconds??0;},
  /** Re-read through the same validator when backup hydration supplies startup-only values. */
  hydrateFromStorage(storage:Pick<Storage,"getItem">){entries=readSaved(storage);},
  set(key:string,seconds:number){
    if(!key||key.length>2048||!Number.isFinite(seconds)||Math.abs(seconds)>30)return;
    const next={...entries};
    if(seconds===0)delete next[key];else next[key]={seconds:Math.round(seconds*10)/10,updated:Date.now()};
    entries=next;
    try{window.localStorage.setItem(STORAGE_KEY,JSON.stringify(entries));}catch{/* Keep session adjustments usable if storage is unavailable. */}
  },
  clear(){entries={};try{window.localStorage.removeItem(STORAGE_KEY);}catch{}},
};
