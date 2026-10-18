import { describe,expect,it,vi } from "vitest";
import { ArtworkDisplayCache } from "./artwork-display";
const url=(n:number)=>`asset://localhost/%2Fown%2Fartwork%2F${n}.jpg`;
const data="data:image/jpeg;base64,/9j/AA==";
describe("display-only artwork cache",()=>{
  it("recovers only local asset URLs without mutating track metadata",async()=>{
    const cache=new ArtworkDisplayCache();const track={id:"one",artworkUrl:url(1)};const before=JSON.stringify(track);const load=vi.fn(async()=>data);
    expect(await cache.recover(track.artworkUrl,load,()=>true)).toBe(data);expect(load).toHaveBeenCalledWith("/own/artwork/1.jpg");expect(JSON.stringify(track)).toBe(before);
    expect(await cache.recover("https://private.example/image.jpg",load,()=>true)).toBeNull();expect(load).toHaveBeenCalledTimes(1);
  });
  it("suppresses stale caller display while retaining valid bytes for a later current caller",async()=>{
    const cache=new ArtworkDisplayCache();let resolve!:(value:string)=>void;let current=true;
    const load=vi.fn(()=>new Promise<string>(done=>{resolve=done;}));
    const pending=cache.recover(url(1),load,()=>current);await Promise.resolve();current=false;resolve(data);
    expect(await pending).toBeNull();expect(cache.get(url(1))).toBe(data);
    expect(await cache.recover(url(1),load,()=>true)).toBe(data);expect(load).toHaveBeenCalledTimes(1);
  });
  it("shares valid bytes with a current second caller when the first caller becomes stale",async()=>{
    const cache=new ArtworkDisplayCache();let resolve!:(value:string)=>void;let firstCurrent=true;
    const load=vi.fn(()=>new Promise<string>(done=>{resolve=done;}));
    const first=cache.recover(url(1),load,()=>firstCurrent);await Promise.resolve();firstCurrent=false;
    const second=cache.recover(url(1),load,()=>true);resolve(data);
    expect(await first).toBeNull();expect(await second).toBe(data);expect(load).toHaveBeenCalledTimes(1);
  });
  it("deduplicates requests, retains at most three and clears in-flight completions",async()=>{
    const cache=new ArtworkDisplayCache();const load=vi.fn(async()=>data);
    await Promise.all([cache.recover(url(1),load,()=>true),cache.recover(url(1),load,()=>true)]);expect(load).toHaveBeenCalledTimes(1);
    for(let n=2;n<=4;n++)await cache.recover(url(n),load,()=>true);
    expect(Object.keys(cache.snapshot())).toHaveLength(3);expect(cache.get(url(1))).toBeUndefined();
    let resolve!:(value:string)=>void;const pending=cache.recover(url(5),()=>new Promise<string>(done=>{resolve=done;}),()=>true);await Promise.resolve();cache.clear();resolve(data);expect(await pending).toBeNull();expect(cache.snapshot()).toEqual({});
  });
  it("rejects non-image and oversized command responses",async()=>{
    const cache=new ArtworkDisplayCache();expect(await cache.recover(url(1),async()=>"private text",()=>true)).toBeNull();
    expect(await cache.recover(url(2),async()=>data+"A".repeat(1_398_150),()=>true)).toBeNull();expect(cache.snapshot()).toEqual({});
  });
});

describe("bounded artwork readmission",()=>{
 it("reloads a successful evicted URL without retaining more than three images",async()=>{
  const cache=new ArtworkDisplayCache(),load=vi.fn(async()=>data);
  for(let n=1;n<=4;n++)await cache.recover(url(n),load,()=>true);
  expect(cache.get(url(1))).toBeUndefined();expect(await cache.recover(url(1),load,()=>true)).toBe(data);expect(load).toHaveBeenCalledTimes(5);expect(Object.keys(cache.snapshot())).toHaveLength(3);
 });
 it("admits a current fourth caller after capacity frees and coalesces its second consumer",async()=>{
  const cache=new ArtworkDisplayCache(),release=new Map<string,(value:string)=>void>();let active=0,maximum=0;
  const load=vi.fn((path:string)=>{active++;maximum=Math.max(maximum,active);return new Promise<string>(resolve=>release.set(path,resolve)).finally(()=>{active--;});});
  const first=[1,2,3].map(n=>cache.recover(url(n),load,()=>true));await Promise.resolve();
  let firstCurrent=true;const fourth=cache.recover(url(4),load,()=>firstCurrent),shared=cache.recover(url(4),load,()=>true);firstCurrent=false;
  expect(load).toHaveBeenCalledTimes(3);release.get('/own/artwork/1.jpg')!(data);await first[0];
  for(let n=0;n<8;n++)await Promise.resolve();expect(load).toHaveBeenCalledTimes(4);expect(maximum).toBe(3);
  release.get('/own/artwork/4.jpg')!(data);expect(await fourth).toBeNull();expect(await shared).toBe(data);
  for(const n of [2,3])release.get(`/own/artwork/${n}.jpg`)!(data);await Promise.all(first);expect(Object.keys(cache.snapshot())).toHaveLength(3);
 });
 it("drops stale or cleared capacity waiters and keeps real failure tombstones",async()=>{
  const cache=new ArtworkDisplayCache();const release:((value:string)=>void)[]=[];const load=vi.fn(()=>new Promise<string>(resolve=>release.push(resolve)));
  const active=[1,2,3].map(n=>cache.recover(url(n),load,()=>true));await Promise.resolve();let current=true;
  const stale=cache.recover(url(4),load,()=>current),cleared=cache.recover(url(5),load,()=>true);current=false;cache.clear();release.forEach(resolve=>resolve(data));await Promise.all(active);expect(await stale).toBeNull();expect(await cleared).toBeNull();expect(load).toHaveBeenCalledTimes(3);expect(cache.snapshot()).toEqual({});
  const failed=vi.fn(async()=>{throw Error('fixture');});expect(await cache.recover(url(9),failed,()=>true)).toBeNull();expect(await cache.recover(url(9),failed,()=>true)).toBeNull();expect(failed).toHaveBeenCalledTimes(1);
 });
});

it("protects at most two active surfaces during unrelated successful writes",async()=>{
 let active=[url(1),url(2)];const cache=new ArtworkDisplayCache(()=>active),load=vi.fn(async()=>data);
 for(let n=1;n<=6;n++)await cache.recover(url(n),load,()=>true);
 expect(cache.get(url(1))).toBe(data);expect(cache.get(url(2))).toBe(data);expect(Object.keys(cache.snapshot())).toHaveLength(3);
 active=[url(6)];await cache.recover(url(7),load,()=>true);expect(cache.get(url(1))).toBeUndefined();expect(Object.keys(cache.snapshot())).toHaveLength(3);
});
