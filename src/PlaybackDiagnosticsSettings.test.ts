// @vitest-environment jsdom
// @ts-expect-error Exercise Svelte's browser runtime in jsdom.
import { mount, tick, unmount } from "../node_modules/svelte/src/index-client.js";
import { afterEach, describe, expect, it, vi } from "vitest";
// @ts-expect-error Svelte internal client runtime has no public declarations
vi.mock("svelte", async () => await import("../node_modules/svelte/src/index-client.js"));
import PlaybackDiagnosticsSettings from "./PlaybackDiagnosticsSettings.svelte";

let destroy: (() => void) | undefined;
afterEach(() => { destroy?.(); destroy = undefined; document.body.innerHTML = ""; vi.restoreAllMocks(); });
async function settle() { for (let i = 0; i < 8; i++) await Promise.resolve(); await tick(); }

describe("PlaybackDiagnosticsSettings", () => {
  it("loads bounded event status and delegates the opt-in toggle", async () => {
    const call = vi.fn(async (command: string) => {
      if (command === "get_playback_diagnostics_status") return { enabled: false, eventCount: 7, maxEvents: 500 };
      return undefined;
    });
    const onEnabledChange = vi.fn();
    const target = document.createElement("div"); document.body.append(target);
    const instance = mount(PlaybackDiagnosticsSettings, { target, props: { enabled: false, onEnabledChange, call } as never });
    destroy = () => unmount(instance); await settle();
    expect(target.textContent).toContain("7 of 500 source events stored");
    expect(target.querySelector("details.report-details")?.hasAttribute("open")).toBe(false);
    expect(target.textContent).toContain("reproduce the playback problem");
    const toggle = target.querySelector<HTMLInputElement>("[aria-label='Enable playback diagnostics']")!;
    toggle.click(); await settle();
    expect(onEnabledChange).toHaveBeenCalledWith(true);
    expect(call).toHaveBeenCalledWith("get_playback_diagnostics_status");
  });

  it("includes optional comparisons in the report", async () => {
    const call = vi.fn(async (command: string) => command === "export_playback_diagnostics" ? {cancelled:true} : {enabled:true,eventCount:0,maxEvents:500});
    const target = document.createElement("div"); document.body.append(target);
    const instance = mount(PlaybackDiagnosticsSettings, {target, props:{enabled:true,onEnabledChange:vi.fn(),call} as never});
    destroy = () => unmount(instance); await settle();
    const browser = target.querySelector<HTMLSelectElement>("[aria-label='Same source in browser']")!;
    browser.value = "fails"; browser.dispatchEvent(new Event("change", {bubbles:true})); await settle();
    Array.from(target.querySelectorAll<HTMLButtonElement>("button")).find(button => button.textContent?.includes("Export report"))!.click(); await settle();
    expect(call).toHaveBeenCalledWith("export_playback_diagnostics", {testerObservations:{browserPlayback:"fails",alternateNetworkPlayback:"not_tested",vpn:"unknown"}});
  });

  it("exports Native diagnostics and clears the backend ring", async () => {
    const call=vi.fn(async(command:string)=>command==="get_playback_diagnostics_status"?{enabled:true,eventCount:2,maxEvents:500}:command==="export_playback_diagnostics"?{cancelled:false,path:"/tmp/report.json",eventCount:2}:undefined);
    const target=document.createElement("div");document.body.append(target);
    const instance=mount(PlaybackDiagnosticsSettings,{target,props:{enabled:true,onEnabledChange:vi.fn(),call}});destroy=()=>unmount(instance);await settle();
    [...target.querySelectorAll<HTMLButtonElement>("button")].find(button=>button.textContent?.includes("Export report"))!.click();await settle();
    expect(call).toHaveBeenCalledWith("export_playback_diagnostics",{testerObservations:null});
    [...target.querySelectorAll<HTMLButtonElement>("button")].find(button=>button.textContent?.includes("Clear diagnostic events"))!.click();await settle();
    expect(call).toHaveBeenCalledWith("clear_playback_diagnostics");
  });
  it("shows bounded Native history and can clear it without source events", async () => {
    const call=vi.fn(async(command:string)=>command==="get_playback_diagnostics_status"?{enabled:true,eventCount:0,maxEvents:2000,droppedEvents:5,nativeEventCount:3,maxNativeEvents:256,droppedNativeEvents:2}:undefined);
    const target=document.createElement("div");document.body.append(target);
    const instance=mount(PlaybackDiagnosticsSettings,{target,props:{enabled:true,onEnabledChange:vi.fn(),call}});destroy=()=>unmount(instance);await settle();
    expect(target.textContent).toContain("3 of 256 Native timeline events");
    expect(target.textContent).toContain("5 older source events dropped");
    expect(target.textContent).toContain("2 older Native events dropped");
    const clear=[...target.querySelectorAll<HTMLButtonElement>("button")].find(button=>button.textContent?.includes("Clear diagnostic events"))!;
    expect(clear.disabled).toBe(false);clear.click();await settle();
    expect(call).toHaveBeenCalledWith("clear_playback_diagnostics");
  });

});
