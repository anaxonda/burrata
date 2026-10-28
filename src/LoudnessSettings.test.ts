// @vitest-environment jsdom
// @ts-expect-error Exercise Svelte's browser runtime in jsdom.
import { mount, tick, unmount } from "../node_modules/svelte/src/index-client.js";
import { afterEach, describe, expect, it, vi } from "vitest";
// @ts-expect-error Svelte internal client runtime has no public declarations
vi.mock("svelte", async () => await import("../node_modules/svelte/src/index-client.js"));
import LoudnessSettings from "./LoudnessSettings.svelte";

const mocks = vi.hoisted(() => ({ call: vi.fn(async () => undefined) }));
vi.mock("./lib/api", () => ({ call: mocks.call }));

let destroy: (() => void) | undefined;
afterEach(() => { destroy?.(); destroy = undefined; document.body.innerHTML = ""; vi.restoreAllMocks(); });
async function settle() { for (let i = 0; i < 8; i++) await Promise.resolve(); await tick(); }

describe("LoudnessSettings", () => {
  it("does not report an applied gain before native playback selects normalization", async () => {
    const target = document.createElement("div"); document.body.append(target);
    const instance = mount(LoudnessSettings, { target, props: {
      settings: { mode: "track", targetLufs: -18, ceilingDbtp: -1, maximumBoostDb: 6, albumTrackFallback: true, analyzeWhenIdle: false },
      onSettingsSaved: vi.fn(), engine: "native",
      playbackStatus: { mode: "off", status: "off", appliedGainDb: 0 },
    } as never });
    destroy = () => unmount(instance); await settle();
    expect(target.querySelector('[role="status"]')?.textContent).toBe("Waiting for playback to select normalization.");
    expect(target.textContent).not.toContain("Applied 0.0 dB");
  });

  it("offers Off, Track, and Album and saves the selected mode", async () => {
    const onSettingsSaved = vi.fn();
    const target = document.createElement("div"); document.body.append(target);
    const settings = { mode: "off" as const, targetLufs: -18, ceilingDbtp: -1, maximumBoostDb: 6, albumTrackFallback: true, analyzeWhenIdle: false };
    const instance = mount(LoudnessSettings, { target, props: { settings, onSettingsSaved } as never });
    destroy = () => unmount(instance); await settle();
    expect([...target.querySelectorAll("option")].map(option => option.textContent)).toEqual(["Off", "Track", "Album"]);
    expect(target.textContent).toContain("your player volume separate");
    const select = target.querySelector<HTMLSelectElement>("select")!;
    select.value = "album"; select.dispatchEvent(new Event("change", { bubbles: true })); await settle();
    expect(mocks.call).toHaveBeenCalledWith("set_loudness_settings", { settings: { ...settings, mode: "album" } });
    expect(onSettingsSaved).toHaveBeenCalledWith({ ...settings, mode: "album" });
  });

  it("shows the explicit fallback, missing-measurement, route, and external-player states", async () => {
    const target = document.createElement("div"); document.body.append(target);
    const instance = mount(LoudnessSettings, { target, props: {
      settings: { mode: "album", targetLufs: -18, ceilingDbtp: -1, maximumBoostDb: 6, albumTrackFallback: true, analyzeWhenIdle: false }, onSettingsSaved: vi.fn(),
      playbackStatus: { appliedGainDb: 1.5, status: "album_fallback_track", measurementSource: "ReplayGain track tag" },
      webviewRouteStatus: "Same-origin Web Audio route active.", engine: "webview",
    } as never });
    destroy = () => unmount(instance); await settle();
    expect(target.textContent).toContain("Using track gain because trusted album analysis is unavailable +1.5 dB");
    expect(target.querySelector('[role="status"]')?.textContent).toContain("ReplayGain track tag");

    unmount(instance);
    const unavailable = mount(LoudnessSettings, { target, props: { settings: { mode: "track", targetLufs: -18, ceilingDbtp: -1, maximumBoostDb: 6, albumTrackFallback: true, analyzeWhenIdle: false }, onSettingsSaved: vi.fn(), engine: "webview", webviewRouteStatus: "Normalization is unavailable for this WebView audio source." } as never });
    destroy = () => { unmount(unavailable); };
    await settle();
    expect(target.textContent).toContain("unavailable for this WebView audio source");
  });

  it("persists opt-in idle analysis and exposes progress, cancellation, and retry controls", async () => {
    const target = document.createElement("div"); document.body.append(target);
    const settings = { mode: "track" as const, targetLufs: -18, ceilingDbtp: -1, maximumBoostDb: 6, albumTrackFallback: true, analyzeWhenIdle: false };
    const onSettingsSaved = vi.fn();
    const onCancelAnalysis = vi.fn();
    const onRetryAnalysis = vi.fn();
    const instance = mount(LoudnessSettings, { target, props: {
      settings, onSettingsSaved, onCancelAnalysis, onRetryAnalysis, pendingAnalysisCount: 2,
      analysisJob: { jobId: "job-1", status: "running", total: 2, completed: 1, error: null },
    } as never });
    destroy = () => unmount(instance); await settle();
    const advanced = target.querySelector<HTMLDetailsElement>(".advanced-options")!;
    expect(advanced.querySelector("summary")?.textContent).toBe("Advanced");
    expect(advanced.open).toBe(false);
    expect(advanced.textContent).toContain("Whole-file analysis uses CPU and disk. Waiting keeps playback responsive.");
    advanced.open = true; await settle();
    const idle = target.querySelector<HTMLInputElement>(".idle-option input")!;
    expect(idle.checked).toBe(false);
    expect(advanced.querySelector("label")?.textContent).toContain("Run selected analysis only while paused");
    idle.checked = true; idle.dispatchEvent(new Event("change", { bubbles: true })); await settle();
    expect(mocks.call).toHaveBeenCalledWith("set_loudness_settings", { settings: { ...settings, analyzeWhenIdle: true } });
    expect(onSettingsSaved).toHaveBeenCalledWith({ ...settings, analyzeWhenIdle: true });
    expect(target.querySelector("progress")?.value).toBe(1);
    expect(target.textContent).toContain("2 explicitly selected tracks queued until playback is idle");
    [...target.querySelectorAll<HTMLButtonElement>("button")].find(button => button.textContent?.includes("Cancel loudness analysis"))!.click();
    expect(onCancelAnalysis).toHaveBeenCalledTimes(1);
  });
});
