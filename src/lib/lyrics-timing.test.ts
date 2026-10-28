// @vitest-environment jsdom
import {afterEach,expect,it,vi} from "vitest";
import { applyBackupFrontendPreferences, BACKUP_FRONTEND_PREFERENCE_RECEIPT_KEY } from "./backup-frontend-preferences";
const original=Object.getOwnPropertyDescriptor(window,"localStorage");
afterEach(()=>{if(original)Object.defineProperty(window,"localStorage",original);else Reflect.deleteProperty(window,"localStorage");vi.resetModules();});
function storage(initial:Record<string,string>={}){
 const values=new Map(Object.entries(initial));
 Object.defineProperty(window,"localStorage",{configurable:true,value:{getItem:(key:string)=>values.get(key)??null,setItem:(key:string,value:string)=>values.set(key,value),removeItem:(key:string)=>values.delete(key)}});
 return values;
}
it("restores saved timing after a fresh module load and clears it with Reset",async()=>{
 vi.resetModules();const values=storage();
 const first=(await import("./lyrics-timing.svelte")).lyricsTiming;
 first.set("recording:source-a",-1.7);
 expect(JSON.parse(values.get("burrata.lyrics-timing.v1")!)["recording:source-a"].seconds).toBe(-1.7);
 vi.resetModules();const restored=(await import("./lyrics-timing.svelte")).lyricsTiming;
 expect(restored.get("recording:source-a")).toBe(-1.7);expect(restored.get("recording:source-b")).toBe(0);
 restored.set("recording:source-a",0);expect(JSON.parse(values.get("burrata.lyrics-timing.v1")!)).toEqual({});
});
it("ignores invalid stored offsets and keeps valid source-specific values",async()=>{
 vi.resetModules();storage({"burrata.lyrics-timing.v1":JSON.stringify({valid:{seconds:2.3,updated:1},invalid:{seconds:400,updated:1},malformed:{seconds:"-3",updated:1}})});
 const timing=(await import("./lyrics-timing.svelte")).lyricsTiming;
 expect(timing.get("valid")).toBe(2.3);expect(timing.get("invalid")).toBe(0);expect(timing.get("malformed")).toBe(0);
});
it("does not evict explicitly saved timing adjustments when more songs are corrected",async()=>{
 vi.resetModules();storage();const timing=(await import("./lyrics-timing.svelte")).lyricsTiming;
 timing.set("first-song",-1.7);for(let i=0;i<300;i++)timing.set(`song-${i}`,0.1);
 vi.resetModules();const restored=(await import("./lyrics-timing.svelte")).lyricsTiming;
 expect(restored.get("first-song")).toBe(-1.7);expect(restored.get("song-299")).toBe(0.1);
});
it("hydrates an imported offset from startup values after storage rollback and preserves later edits",async()=>{
 vi.resetModules();
 const key="burrata.lyrics-timing.v1", nonce="10000000-0000-4000-8000-000000000001", session1="20000000-0000-4000-8000-000000000001";
 const oldValue=JSON.stringify({"recording:track":{"seconds":2.4,updated:1}});
 const importedValue=JSON.stringify({"recording:track":{"seconds":-1.3,updated:2}});
 const values=storage({[key]:oldValue});
 const persistence={getItem:(name:string)=>values.get(name)??null,setItem:(name:string,value:string)=>values.set(name,value)};
 const overrides=new Map<string,string>();
 const applied=vi.fn((items:ReadonlyMap<string,string>)=>items.forEach((value,name)=>overrides.set(name,value)));
 const preference={"burrata.lyrics-timing.v1":{"recording:track":{"seconds":-1.3,updated:2}}};
 await applyBackupFrontendPreferences(async()=>({nonce,currentSessionId:session1,preferences:preference}),persistence,async()=>{
   // Immediate readback succeeded, then an existing WebKit value rolled back.
   values.set(key,oldValue); values.delete(BACKUP_FRONTEND_PREFERENCE_RECEIPT_KEY);
   return "deferred";
 },applied);
 expect(values.get(key)).toBe(oldValue);
 const lyrics=(await import("./lyrics-timing.svelte")).lyricsTiming;
 const startupRead={getItem:(name:string)=>{
   if(overrides.has(name)){const value=overrides.get(name)!;overrides.delete(name);return value;}
   return values.get(name)??null;
 }};
 lyrics.hydrateFromStorage(startupRead);
 expect(lyrics.get("recording:track")).toBe(-1.3);
 expect(applied).toHaveBeenCalledOnce();

 // A later boot sees the matching receipt; it must use current user storage.
 const userValue=JSON.stringify({"recording:track":{"seconds":0.8,updated:3}});
 values.set(key,userValue);
 values.set(BACKUP_FRONTEND_PREFERENCE_RECEIPT_KEY,JSON.stringify({nonce,sessionId:session1}));
 const laterApplied=vi.fn();
 await applyBackupFrontendPreferences(async()=>({nonce,currentSessionId:"20000000-0000-4000-8000-000000000002",preferences:preference}),persistence,async()=>"acknowledged",laterApplied);
 expect(laterApplied).not.toHaveBeenCalled();
 lyrics.hydrateFromStorage(persistence);
 expect(lyrics.get("recording:track")).toBe(0.8);
});
