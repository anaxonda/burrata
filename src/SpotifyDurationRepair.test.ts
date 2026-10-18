// @vitest-environment jsdom
// @ts-expect-error Browser runtime for direct Svelte mounts.
import {mount,tick,unmount} from "../node_modules/svelte/src/index-client.js";
import {afterEach,expect,it,vi} from "vitest";
// @ts-expect-error Browser runtime for direct Svelte mounts.
vi.mock("svelte",async()=>await import("../node_modules/svelte/src/index-client.js"));
import SpotifyDurationRepair from "./SpotifyDurationRepair.svelte";
let cleanup:(()=>void)|undefined;
afterEach(()=>{cleanup?.();document.body.innerHTML="";});
async function settle(){for(let i=0;i<10;i++)await Promise.resolve();await new Promise(r=>setTimeout(r,0));await tick();}
function render(call:(command:string)=>Promise<unknown>){const target=document.createElement("div");document.body.append(target);const app=mount(SpotifyDurationRepair,{target,props:{call}});cleanup=()=>void unmount(app);return target;}
it("runs only after an explicit action and reports a partial slice honestly",async()=>{
 const call=vi.fn(async(command:string)=>command==="public_spotify_duration_repair_running"?false:{changed:3,processed:5,complete:false});
 const target=render(call);await settle();expect(call).not.toHaveBeenCalledWith("repair_spotify_durations_public");
 [...target.querySelectorAll<HTMLButtonElement>("button")].find(b=>b.textContent==="Check missing durations")!.click();await settle();
 expect(call).toHaveBeenCalledWith("repair_spotify_durations_public");expect(target.textContent).toContain("Checked 5 tracks; repaired 3 durations");expect(target.textContent).toContain("Run again to continue");
});
it("can stop a repair already running when settings are reopened",async()=>{
 let active=true;const call=vi.fn(async(command:string)=>{if(command==="cancel_public_spotify_duration_repair"){active=false;return true;}return active;});
 const target=render(call);await settle();const stop=[...target.querySelectorAll<HTMLButtonElement>("button")].find(b=>b.textContent==="Stop repair")!;expect(stop.disabled).toBe(false);stop.click();await settle();
 expect(call).toHaveBeenCalledWith("cancel_public_spotify_duration_repair");expect(target.textContent).not.toContain("Duration repair is running.");
});
it("does not claim all durations were repaired when the scan is exhausted",async()=>{
 const call=vi.fn(async(command:string)=>command==="public_spotify_duration_repair_running"?false:{changed:0,processed:1,complete:true});const target=render(call);await settle();target.querySelector<HTMLButtonElement>("button")!.click();await settle();expect(target.textContent).toContain("Unavailable tracks may still have no duration");
});

it("keeps the repair result when status is refreshed during its request",async()=>{
 let finish:((value:{changed:number;processed:number;complete:boolean})=>void)|undefined;
 const call=vi.fn(async(command:string)=>{if(command==="repair_spotify_durations_public")return await new Promise(resolve=>{finish=resolve;});return false;});
 const target=render(call);await settle();target.querySelector<HTMLButtonElement>("button")!.click();await settle();[...target.querySelectorAll<HTMLButtonElement>("button")].find(b=>b.textContent==="Refresh status")!.click();await settle();expect(target.querySelector<HTMLButtonElement>("button")!.disabled).toBe(true);finish?.({changed:2,processed:2,complete:true});await settle();expect(target.textContent).toContain("repaired 2 durations");
});
