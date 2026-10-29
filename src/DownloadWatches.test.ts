// @vitest-environment jsdom
// @ts-expect-error Svelte client runtime for jsdom.
import {mount,tick,unmount} from "../node_modules/svelte/src/index-client.js";
import {afterEach,expect,it,vi} from "vitest";
// @ts-expect-error Internal client runtime.
vi.mock("svelte",async()=>await import("../node_modules/svelte/src/index-client.js"));
const {call}=vi.hoisted(()=>({call:vi.fn()}));
vi.mock("./lib/api",()=>({call,desktop:()=>true,listen:async()=>()=>{}}));
import DownloadWatches from "./DownloadWatches.svelte";
let instance:ReturnType<typeof mount>;
afterEach(async()=>{if(instance)await unmount(instance);document.body.innerHTML="";vi.clearAllMocks();});
const item=(state="waiting",kind="waiting")=>({id:"watch-1",state,kind,nextCheck:1800000000,request:{track:{id:"recording",title:"Recording",artist:"Artist"},requiresLossless:kind==="upgrade"}});
async function settle(){for(let i=0;i<8;i++)await Promise.resolve();await tick();}
it("keeps waiting and lossless checking distinct, with compact menu actions",async()=>{
  call.mockResolvedValue([item(),{...item("waiting","upgrade"),id:"watch-2"}]);
  instance=mount(DownloadWatches,{target:document.body,props:{onReview:vi.fn()}});await settle();
  expect(document.body.textContent).toContain("Waiting for source");expect(document.body.textContent).toContain("Downloaded · looking for lossless");
  const menus=document.querySelectorAll("article details");expect(menus.length).toBe(2);expect([...menus].every(node=>!(node as HTMLDetailsElement).open)).toBe(true);
});
it("pauses an automatic request before opening source selection",async()=>{
  call.mockImplementation(async(command)=>command==="list_download_watches"?[item()]:undefined);
  const review=vi.fn();instance=mount(DownloadWatches,{target:document.body,props:{onReview:review}});await settle();
  const choose=[...document.querySelectorAll("button")].find(button=>button.textContent==="Choose source…")!;choose.click();await settle();
  expect(call).toHaveBeenCalledWith("control_download_watch",{id:"watch-1",action:"pause"});expect(review).toHaveBeenCalledWith(expect.objectContaining({id:"recording"}));
});
it("leaves queued transfers in the existing transfer list",async()=>{
  call.mockResolvedValue([item("queued")]);instance=mount(DownloadWatches,{target:document.body,props:{onReview:vi.fn()}});await settle();
  expect(document.querySelector(".download-watches")).toBeNull();
});
it("dismisses options with Escape, outside clicks, and after an action",async()=>{
  call.mockResolvedValue([item()]);instance=mount(DownloadWatches,{target:document.body,props:{onReview:vi.fn()}});await settle();
  const menu=document.querySelector<HTMLDetailsElement>("article details")!;
  menu.open=true;window.dispatchEvent(new KeyboardEvent("keydown",{key:"Escape"}));expect(menu.open).toBe(false);
  menu.open=true;document.body.click();expect(menu.open).toBe(false);
  menu.open=true;[...menu.querySelectorAll("button")].find(button=>button.textContent==="Pause")!.click();await settle();expect(menu.open).toBe(false);
  expect(call).toHaveBeenCalledWith("control_download_watch",{id:"watch-1",action:"pause"});
});
