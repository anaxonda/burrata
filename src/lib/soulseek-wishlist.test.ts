import { describe, expect, it, vi } from "vitest";
import { SoulseekWishlist, WISHLIST_GAP, WISHLIST_INTERVAL, WISHLIST_KEY } from "./soulseek-wishlist";
function fixture() {
  const data = new Map<string,string>(); let now = 1000;
  const storage = {getItem: (k:string) => data.get(k) ?? null, setItem: (k:string,v:string) => {data.set(k,v);}};
  return {data, storage, now: () => now, advance: (ms:number) => {now += ms;}};
}
describe("Soulseek wishlist scheduling", () => {
  it("persists explicit subscriptions and does not repeat an already reserved search after restart", async () => {
    const f=fixture(), w=new SoulseekWishlist(f.storage,f.now);w.add(" Artist   Track ");w.add("artist track");
    const search=vi.fn(async()=>3);await w.tick(search);
    expect(w.entries).toHaveLength(1);expect(w.entries[0]).toMatchObject({query:"Artist Track",resultCount:3,outcome:"results"});
    const restored=new SoulseekWishlist(f.storage,f.now);await restored.tick(search);expect(search).toHaveBeenCalledTimes(1);
    f.advance(WISHLIST_INTERVAL);await restored.tick(search);expect(search).toHaveBeenCalledTimes(2);
  });
  it("serializes network work and spaces different subscriptions", async () => {
    const f=fixture(), w=new SoulseekWishlist(f.storage,f.now);w.add("one");w.add("two");
    let finish!:(n:number)=>void;const search=vi.fn(()=>new Promise<number>(r=>{finish=r;}));
    const first=w.tick(search);await w.tick(search);expect(search).toHaveBeenCalledTimes(1);finish(0);await first;
    await w.tick(search);expect(search).toHaveBeenCalledTimes(1);f.advance(WISHLIST_GAP);
    const second=w.tick(search);expect(search).toHaveBeenCalledTimes(2);finish(0);await second;
  });
  it("retains the global search gap across restart", async () => {
    const f=fixture(),w=new SoulseekWishlist(f.storage,f.now);w.add("one");w.add("two");
    const search=vi.fn(async()=>0);await w.tick(search);
    const restored=new SoulseekWishlist(f.storage,f.now);await restored.tick(search);expect(search).toHaveBeenCalledTimes(1);
    f.advance(WISHLIST_GAP);await restored.tick(search);expect(search).toHaveBeenCalledTimes(2);
  });
  it("aborts removed searches without resurrecting them or persisting results", async () => {
    const f=fixture(),w=new SoulseekWishlist(f.storage,f.now);w.add("one");let finish!:(n:number)=>void;let signal!:AbortSignal;
    const pending=w.tick(async(_q,s)=>{signal=s;return new Promise(r=>{finish=r;});});w.remove(w.entries[0]!.id);
    expect(signal.aborted).toBe(true);finish(4);await pending;expect(w.entries).toEqual([]);
  });
  it("backs off failures and keeps provider errors out of saved metadata", async()=>{
    const f=fixture(),w=new SoulseekWishlist(f.storage,f.now);w.add("one");
    await w.tick(async()=>{throw new Error("private provider detail");});expect(w.entries[0]).toMatchObject({failures:1,outcome:"unavailable",resultCount:null});
    f.advance(WISHLIST_INTERVAL);await w.tick(async()=>{throw new Error("private provider detail");});
    expect(w.entries[0]!.nextRun).toBe(f.now()+2*WISHLIST_INTERVAL);expect(f.data.get(WISHLIST_KEY)).not.toContain("private provider detail");
  });
  it("starts no work from corrupt storage and reports persistence failure before searching",async()=>{
    const f=fixture();f.data.set(WISHLIST_KEY,'{"version":1,"entries":[{"enabled":true}]}');
    const w=new SoulseekWishlist(f.storage,f.now);expect(w.entries).toEqual([]);w.add("one");
    f.storage.setItem=()=>{throw new Error("storage full");};const search=vi.fn(async()=>0);
    await expect(w.tick(search)).rejects.toThrow("storage full");expect(search).not.toHaveBeenCalled();
  });
});
