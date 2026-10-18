import { describe, expect, it, vi } from "vitest";
import { NativePlaybackBridge } from "./native-playback";
import type { NativePlaybackSnapshot, Track } from "./types";

const track: Track = { id:"a", provider:"local", providerId:"a", sourceUrl:"file:///a", artist:"Artist", title:"Track", favorite:false, provenance:{} };
function snapshot(revision:number,generation=2):NativePlaybackSnapshot {
  return {engine:"native",revision,generation,status:"playing",trackId:"a",track,candidate:null,queue:[track],currentIndex:0,paused:false,position:0,duration:10,volume:.8,canSeek:true,canVolume:true,shuffle:false,repeat:"off",playerMessage:"",quality:{}};
}

describe("NativePlaybackBridge", () => {
  it("retains the same queue array for state-only events and replaces it on queue changes", async () => {
    const initial={...snapshot(3),queueRevision:1};
    const invoke=vi.fn(async()=>initial);
    const listen=vi.fn(async()=>()=>{});
    const bridge=new NativePlaybackBridge(invoke as unknown as ConstructorParameters<typeof NativePlaybackBridge>[0],listen as unknown as ConstructorParameters<typeof NativePlaybackBridge>[1]);
    await bridge.initialize(()=>{},()=>{});
    const queue=bridge.snapshot!.queue;
    const {queue:omitted,...delta}={...snapshot(4),queueRevision:1,status:"buffering" as const};
    expect(bridge.acceptSnapshot(delta)).toBe(true);
    expect(bridge.snapshot!.queue).toBe(queue);
    expect(bridge.snapshot!.status).toBe("buffering");
    const replacement={...snapshot(5),queueRevision:2,queue:[{...track,id:"new"}]};
    bridge.acceptSnapshot(replacement);
    expect(bridge.snapshot!.queue).toBe(replacement.queue);
  });

  it("merges a state-only event arriving before the initial queue response", async () => {
    const handlers=new Map<string,(event:{payload:unknown})=>void>();
    const initial={...snapshot(3),queueRevision:1};
    const invoke=vi.fn(async()=>{
      const {queue:omitted,...delta}={...snapshot(4),queueRevision:1,status:"paused" as const};
      handlers.get("native-playback-state")?.({payload:delta});
      return initial;
    });
    const listen=vi.fn(async<T>(event:string,handler:(event:{payload:T})=>void)=>{handlers.set(event,handler as (event:{payload:unknown})=>void);return ()=>{};});
    const bridge=new NativePlaybackBridge(invoke as unknown as ConstructorParameters<typeof NativePlaybackBridge>[0],listen as unknown as ConstructorParameters<typeof NativePlaybackBridge>[1]);
    const states:NativePlaybackSnapshot[]=[];
    const result=await bridge.initialize(s=>states.push(s),()=>{});
    expect(result.revision).toBe(4);
    expect(result.status).toBe("paused");
    expect(result.queue).toBe(initial.queue);
    expect(states.map(s=>s.revision)).toEqual([4]);
  });

  it("uses the native command envelope and ignores stale state and position events", async () => {
    const handlers = new Map<string,(event:{payload:unknown})=>void>();
    const invoke = vi.fn(async <T>(command:string,_args?:Record<string,unknown>) => (command === "get_playback_snapshot" ? snapshot(3) : undefined) as T);
    const listen = vi.fn(async <T>(event:string,handler:(event:{payload:T})=>void) => { handlers.set(event,handler as (event:{payload:unknown})=>void); return vi.fn(); });
    const bridge = new NativePlaybackBridge(invoke as unknown as ConstructorParameters<typeof NativePlaybackBridge>[0],listen as unknown as ConstructorParameters<typeof NativePlaybackBridge>[1]);
    const states:NativePlaybackSnapshot[]=[]; const positions:number[]=[];
    await bridge.initialize(value=>states.push(value),value=>positions.push(value.position));
    handlers.get("native-playback-state")?.({payload:snapshot(4)});
    handlers.get("native-playback-state")?.({payload:snapshot(2)});
    handlers.get("native-playback-position")?.({payload:{generation:1,position:9}});
    handlers.get("native-playback-position")?.({payload:{generation:2,revision:5,position:5,chapter:{title:"Verse",startSeconds:30,endSeconds:40},quality:{codec:"FLAC"}}});
    await bridge.command({action:"seek",position:5});
    expect(states.map(value=>value.revision)).toEqual([3,4]);
    expect(positions).toEqual([5]);
    expect(bridge.snapshot).toMatchObject({revision:5,chapter:{title:"Verse"},quality:{codec:"FLAC"}});
    expect(invoke).toHaveBeenLastCalledWith("playback_command",{command:{action:"seek",position:5}});
  });

  it("subscribes before fetching and keeps a newer event over the initial response", async () => {
    const handlers = new Map<string,(event:{payload:unknown})=>void>();
    const invoke = vi.fn(async <T>(command:string) => {
      if (command === "get_playback_snapshot") {
        handlers.get("native-playback-state")?.({payload:snapshot(8)});
        return snapshot(6) as T;
      }
      return undefined as T;
    });
    const listen = vi.fn(async <T>(event:string,handler:(event:{payload:T})=>void) => {
      handlers.set(event,handler as (event:{payload:unknown})=>void);
      return vi.fn();
    });
    const bridge = new NativePlaybackBridge(invoke as unknown as ConstructorParameters<typeof NativePlaybackBridge>[0],listen as unknown as ConstructorParameters<typeof NativePlaybackBridge>[1]);
    const revisions:number[]=[];
    const result=await bridge.initialize(value=>revisions.push(value.revision),()=>{});
    expect(listen.mock.invocationCallOrder[0]).toBeLessThan(invoke.mock.invocationCallOrder[0]!);
    expect(result.revision).toBe(8);
    expect(bridge.snapshot?.revision).toBe(8);
    expect(revisions).toEqual([8]);
  });

  it("applies a newer initial response when an older event arrives during the query", async () => {
    const handlers = new Map<string,(event:{payload:unknown})=>void>();
    const invoke = vi.fn(async <T>(command:string) => {
      if (command === "get_playback_snapshot") {
        handlers.get("native-playback-state")?.({payload:snapshot(6)});
        return snapshot(8) as T;
      }
      return undefined as T;
    });
    const listen = vi.fn(async <T>(event:string,handler:(event:{payload:T})=>void) => {
      handlers.set(event,handler as (event:{payload:unknown})=>void);
      return vi.fn();
    });
    const bridge = new NativePlaybackBridge(invoke as unknown as ConstructorParameters<typeof NativePlaybackBridge>[0],listen as unknown as ConstructorParameters<typeof NativePlaybackBridge>[1]);
    const revisions:number[]=[];
    const result=await bridge.initialize(value=>revisions.push(value.revision),()=>{});
    expect(result.revision).toBe(8);
    expect(bridge.snapshot?.revision).toBe(8);
    expect(revisions).toEqual([6,8]);
  });

  it("cleans up an earlier listener when a later subscription fails", async () => {
    const stopState=vi.fn();
    const invoke=vi.fn(async <T>()=>snapshot(1) as T);
    const listen=vi.fn(async <T>(event:string,_handler:(event:{payload:T})=>void)=>{
      if(event==="native-playback-position")throw new Error("event connection failed");
      return stopState;
    });
    const bridge=new NativePlaybackBridge(invoke as unknown as ConstructorParameters<typeof NativePlaybackBridge>[0],listen as unknown as ConstructorParameters<typeof NativePlaybackBridge>[1]);
    await expect(bridge.initialize(()=>{},()=>{})).rejects.toThrow("event connection failed");
    expect(stopState).toHaveBeenCalledOnce();
    expect(bridge.snapshot).toBeNull();
  });
});
