// @vitest-environment jsdom
// @ts-expect-error Svelte browser runtime.
import {mount,tick,unmount} from "../node_modules/svelte/src/index-client.js";
import {afterEach,beforeEach,expect,it,vi} from "vitest";
import SoulseekWishlist from "./SoulseekWishlist.svelte";
import {WISHLIST_KEY} from "./lib/soulseek-wishlist";
const mocks=vi.hoisted(()=>({call:vi.fn()}));
vi.mock("./lib/api",()=>({call:mocks.call,desktop:()=>true}));
vi.mock("svelte",async()=>{
  // @ts-expect-error Match Svelte browser runtime.
  return await import("../node_modules/svelte/src/index-client.js");
});
let instance:ReturnType<typeof mount>|null=null;
beforeEach(()=>{
  const storage=new Map<string,string>();
  vi.stubGlobal("localStorage",{getItem:(key:string)=>storage.get(key)??null,setItem:(key:string,value:string)=>storage.set(key,value)});
  mocks.call.mockReset();
});
afterEach(async()=>{if(instance)await unmount(instance);instance=null;document.body.innerHTML="";vi.unstubAllGlobals();});
it("saves only an explicit subscription, offers review, pause and remove",async()=>{
  const review=vi.fn();instance=mount(SoulseekWishlist,{target:document.body,props:{visible:true,query:"Artist Track",onReview:review}});await tick();
  expect(mocks.call).not.toHaveBeenCalled();
  [...document.querySelectorAll('button')].find(b=>b.textContent?.includes('Save search'))!.click();await tick();
  expect(JSON.parse(localStorage.getItem(WISHLIST_KEY)!).entries).toHaveLength(1);
  [...document.querySelectorAll('button')].find(b=>b.textContent==='Search now')!.click();expect(review).toHaveBeenCalledWith('Artist Track');
  const checkbox=document.querySelector<HTMLInputElement>('input[type=checkbox]')!;checkbox.checked=false;checkbox.dispatchEvent(new Event('change',{bubbles:true}));await tick();
  expect(JSON.parse(localStorage.getItem(WISHLIST_KEY)!).entries[0].enabled).toBe(false);
  document.querySelector<HTMLButtonElement>('[aria-label="Remove wishlist search Artist Track"]')!.click();await tick();
  expect(JSON.parse(localStorage.getItem(WISHLIST_KEY)!).entries).toEqual([]);
  expect(mocks.call).not.toHaveBeenCalled();
});
it("does not render controls while hidden",async()=>{
  instance=mount(SoulseekWishlist,{target:document.body,props:{visible:false,query:'track'}});await tick();
  expect(document.querySelector('details')).toBeNull();expect(mocks.call).not.toHaveBeenCalled();
});
