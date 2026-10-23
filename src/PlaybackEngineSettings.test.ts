// @vitest-environment jsdom
// @ts-expect-error Use Svelte browser runtime.
import {mount,tick,unmount} from "../node_modules/svelte/src/index-client.js";
import {afterEach,expect,it,vi} from "vitest";
const mocks=vi.hoisted(()=>({call:vi.fn()}));
vi.mock("./lib/api",()=>({call:mocks.call,desktop:()=>true}));
// @ts-expect-error Match Svelte browser runtime.
vi.mock("svelte",async()=>await import("../node_modules/svelte/src/index-client.js"));
import PlaybackEngineSettings from "./PlaybackEngineSettings.svelte";
let instance:ReturnType<typeof mount>|null=null;
afterEach(async()=>{if(instance)await unmount(instance);instance=null;document.body.innerHTML="";mocks.call.mockReset();});
async function settle(){for(let i=0;i<8;i++)await Promise.resolve();await tick();}
it("keeps selection as a draft until explicit apply and restart",async()=>{
  const restart=vi.fn().mockResolvedValue(undefined);
  mocks.call.mockImplementation(async(command:string)=>command==="get_playback_engine_settings"?{selected:"webview",active:"webview",locked:false,restartRequired:false}:{selected:"native",active:"webview",locked:false,restartRequired:true});
  instance=mount(PlaybackEngineSettings,{target:document.body,props:{onApplyRestart:restart}});await settle();
  const select=document.querySelector<HTMLSelectElement>('select')!;expect(select.disabled).toBe(false);
  select.value="native";select.dispatchEvent(new Event("change",{bubbles:true}));await settle();
  expect(mocks.call).toHaveBeenCalledTimes(1);expect(restart).not.toHaveBeenCalled();
  document.querySelector<HTMLButtonElement>("button")!.click();await settle();
  expect(restart).toHaveBeenCalledWith("native");
});
it("keeps fixed-launch settings read-only and does not write preferences",async()=>{
  mocks.call.mockResolvedValue({selected:"native",active:"native",locked:true,restartRequired:false});
  instance=mount(PlaybackEngineSettings,{target:document.body,props:{onApplyRestart:vi.fn()}});await settle();
  expect(document.querySelector<HTMLSelectElement>('select')!.disabled).toBe(true);
  expect(mocks.call).toHaveBeenCalledTimes(1);expect(document.body.textContent).toContain("fixed audio engine");
});
it("retains the saved choice and reports a failed handoff",async()=>{
  mocks.call.mockImplementation(async(command:string)=>{if(command==="get_playback_engine_settings")return{selected:"webview",active:"webview",locked:false,restartRequired:false};throw new Error("unavailable");});
  instance=mount(PlaybackEngineSettings,{target:document.body,props:{onApplyRestart:vi.fn().mockRejectedValue(new Error("save failed"))}});await settle();
  const select=document.querySelector<HTMLSelectElement>('select')!;select.value="native";select.dispatchEvent(new Event("change",{bubbles:true}));await settle();
  document.querySelector<HTMLButtonElement>("button")!.click();await settle();
  expect(select.value).toBe("webview");expect(document.body.textContent).toContain("Could not switch");
});
