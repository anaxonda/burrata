import {afterEach,describe,expect,it,vi} from "vitest";
import {searchWishlist,type WishlistCall} from "./soulseek-wishlist-search";
afterEach(()=>vi.useRealTimers());
describe("wishlist native search lifecycle",()=>{
  it("counts distinct candidates and cleans up without acquiring files",async()=>{
    const call=vi.fn(async(command:string)=>command==="start_slskd_search"?{id:"search-1"}:command==="get_slskd_search"?{complete:true,candidates:[{id:"a"},{id:"a"},{id:"b"}]}:null);
    expect(await searchWishlist(call as WishlistCall,"artist track",new AbortController().signal)).toBe(2);
    expect(call.mock.calls.map(c=>c[0])).toEqual(["start_slskd_search","get_slskd_search","cancel_slskd_search"]);
  });
  it("cancels an ID returned after the subscription was removed",async()=>{
    let finish!:(value:unknown)=>void;
    const call=vi.fn((command:string)=>command==="start_slskd_search"?new Promise(r=>{finish=r;}):Promise.resolve(null));
    const controller=new AbortController();const pending=searchWishlist(call as WishlistCall,"track",controller.signal);
    controller.abort();finish({id:"late-search"});await expect(pending).rejects.toBeDefined();
    expect(call.mock.calls.map(c=>c[0])).toEqual(["start_slskd_search","cancel_slskd_search"]);
  });
  it("cleans up a failed poll and does not describe an outage as an empty search",async()=>{
    const call=vi.fn(async(command:string)=>{
      if(command==="start_slskd_search")return{id:"search-1"};
      if(command==="get_slskd_search")return{complete:true,candidates:[],warnings:["unavailable"]};return null;
    });
    await expect(searchWishlist(call as WishlistCall,"track",new AbortController().signal)).rejects.toThrow("Search incomplete");
    expect(call.mock.calls[call.mock.calls.length-1]?.[0]).toBe("cancel_slskd_search");
  });
  it("bounds observation and cancels a search that never completes",async()=>{
    vi.useFakeTimers();
    const call=vi.fn(async(command:string)=>command==="start_slskd_search"?{id:"search-1"}:command==="get_slskd_search"?{complete:false,candidates:[]}:null);
    const pending=searchWishlist(call as WishlistCall,"track",new AbortController().signal);
    const rejection=expect(pending).rejects.toThrow("deadline");await vi.advanceTimersByTimeAsync(31_000);await rejection;
    expect(call.mock.calls.filter(c=>c[0]==="get_slskd_search")).toHaveLength(30);
    expect(call.mock.calls[call.mock.calls.length-1]?.[0]).toBe("cancel_slskd_search");
  });
});
