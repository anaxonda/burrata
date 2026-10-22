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
    expect(target.textContent).toContain("7 of 500 diagnostic events stored");
    expect(target.textContent).toContain("exclude credentials and full source URLs");
    const toggle = target.querySelector<HTMLInputElement>("[aria-label='Enable playback diagnostics']")!;
    toggle.click(); await settle();
    expect(onEnabledChange).toHaveBeenCalledWith(true);
    expect(call).toHaveBeenCalledWith("get_playback_diagnostics_status");
  });

  it("clears explicitly and exports through the native save-dialog command", async () => {
    const call = vi.fn(async (command: string) => {
      if (command === "get_playback_diagnostics_status") return { enabled: true, eventCount: 2, maxEvents: 500 };
      if (command === "export_playback_diagnostics") return { cancelled: false, path: "/tmp/playback-diagnostics.json", eventCount: 2 };
      return undefined;
    });
    const target = document.createElement("div"); document.body.append(target);
    const instance = mount(PlaybackDiagnosticsSettings, { target, props: { enabled: true, onEnabledChange: vi.fn(), call } as never });
    destroy = () => unmount(instance); await settle();
    target.querySelector<HTMLButtonElement>("[type=button]:nth-of-type(2)")!.click(); await settle();
    expect(call).toHaveBeenCalledWith("clear_playback_diagnostics");
    expect(target.textContent).toContain("Playback diagnostic events cleared");
    target.querySelector<HTMLButtonElement>("[type=button]:nth-of-type(3)")!.click(); await settle();
    expect(call).toHaveBeenCalledWith("export_playback_diagnostics");
    expect(target.textContent).toContain("Exported 2 playback diagnostic events to /tmp/playback-diagnostics.json");
  });
});
