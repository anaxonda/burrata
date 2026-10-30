// @vitest-environment jsdom
// @ts-expect-error Exercise Svelte's browser runtime in jsdom.
import { mount, tick, unmount } from "../node_modules/svelte/src/index-client.js";
import { afterEach, describe, expect, it, vi } from "vitest";
// @ts-expect-error Svelte internal client runtime has no public declarations.
vi.mock("svelte", async () => await import("../node_modules/svelte/src/index-client.js"));
import StreamMediaAnalysisSettings from "./StreamMediaAnalysisSettings.svelte";

let destroy: (() => void) | undefined;
afterEach(() => { destroy?.(); destroy = undefined; document.body.innerHTML = ""; });

describe("StreamMediaAnalysisSettings", () => {
  it("defaults visibly off and explains the bounded background work", async () => {
    const target = document.createElement("div"); document.body.append(target);
    const instance = mount(StreamMediaAnalysisSettings, { target, props: { enabled: false, onEnabledChange: vi.fn() } });
    destroy = () => unmount(instance);
    await tick();
    const toggle = target.querySelector<HTMLInputElement>("input[aria-label='Analyze full streamed tracks in background']")!;
    expect(toggle.checked).toBe(false);
    expect(target.textContent).toContain("while you listen");
    expect(target.textContent).toContain("Audio is not saved");
  });

  it("reports current analysis state without replacing the opt-in control", async () => {
    const target = document.createElement("div"); document.body.append(target);
    const instance = mount(StreamMediaAnalysisSettings, { target, props: {
      enabled: true, status: "Analyzing verified stream · 1 of 1", onEnabledChange: vi.fn(),
    } });
    destroy = () => unmount(instance);
    await tick();
    expect(target.querySelector("input")?.checked).toBe(true);
    expect(target.querySelector("[role='status']")?.textContent).toContain("1 of 1");
  });

  it("shows startup failures once and keeps a visible retry action", async () => {
    const target = document.createElement("div"); document.body.append(target);
    const message = "Stream analysis unavailable: Current streamed-track metadata is invalid";
    const onMediaStart = vi.fn();
    const instance = mount(StreamMediaAnalysisSettings, { target, props: {
      enabled: true, onEnabledChange: vi.fn(), mediaEligible: true,
      mediaState: "idle", mediaStatus: "", status: message, onMediaStart,
    } });
    destroy = () => unmount(instance);
    await tick();
    expect(target.querySelectorAll("[role='status']")).toHaveLength(1);
    expect(target.querySelector("[role='status']")?.textContent).toBe(message);
    const retry = target.querySelector<HTMLButtonElement>("button")!;
    expect(retry.textContent).toContain("Retry analysis");
    retry.click();
    expect(onMediaStart).toHaveBeenCalledOnce();
  });

  it("reports preference changes", async () => {
    const target = document.createElement("div"); document.body.append(target);
    const onEnabledChange = vi.fn();
    const instance = mount(StreamMediaAnalysisSettings, { target, props: { enabled: false, onEnabledChange } });
    destroy = () => unmount(instance);
    await tick();
    target.querySelector<HTMLInputElement>("input")!.click();
    expect(onEnabledChange).toHaveBeenCalledWith(true);
  });

  it("keeps stream analysis start and cancel controls in Settings outside the background checkbox", async () => {
    const target = document.createElement("div"); document.body.append(target);
    const onMediaCancel = vi.fn();
    const onObservedCancel = vi.fn();
    const instance = mount(StreamMediaAnalysisSettings, { target, props: {
      enabled: true, onEnabledChange: vi.fn(),
      mediaEligible: false, mediaState: "running", onMediaCancel,
      observedEligible: true, observedState: "capturing", onObservedCancel,
    } });
    destroy = () => unmount(instance);
    await tick();

    const backgroundLabel = target.querySelector("label.stream-media-analysis-setting")!;
    const mediaCancel = target.querySelector<HTMLButtonElement>('[aria-label="Cancel faster-than-realtime full stream analysis"]')!;
    const observedCancel = target.querySelector<HTMLButtonElement>('[aria-label="Cancel whole-track BPM and key analysis"]')!;
    expect(backgroundLabel.contains(mediaCancel)).toBe(false);
    expect(backgroundLabel.contains(observedCancel)).toBe(false);
    mediaCancel.click();
    observedCancel.click();
    expect(onMediaCancel).toHaveBeenCalledOnce();
    expect(onObservedCancel).toHaveBeenCalledOnce();
  });

  it("explains automatic cached local waveforms separately from stream opt-in", async () => {
    const target = document.createElement("div"); document.body.append(target);
    const onLocalStart = vi.fn();
    const instance = mount(StreamMediaAnalysisSettings, { target, props: {
      enabled: false, onEnabledChange: vi.fn(), localEligible: true, localStatus: "",
      onLocalStart,
    } });
    destroy = () => unmount(instance);
    await tick();

    const streamOptIn = target.querySelector("label.stream-media-analysis-setting")!;
    const localControl = target.querySelector(".local-waveform-analysis-control")!;
    expect(localControl).not.toBeNull();
    expect(streamOptIn.contains(localControl)).toBe(false);
    expect(localControl.textContent).toContain("analyzed automatically");
    expect(localControl.textContent).toContain("reused later");
    const start = localControl.querySelector<HTMLButtonElement>('[aria-label="Analyze full local waveform"]')!;
    expect(start.textContent).toContain("Analyze full file waveform");
    start.click();
    expect(onLocalStart).toHaveBeenCalledOnce();
  });

  it("uses a clear ready label for a cached complete local waveform", async () => {
    const target = document.createElement("div"); document.body.append(target);
    const instance = mount(StreamMediaAnalysisSettings, { target, props: {
      enabled: false, onEnabledChange: vi.fn(), localEligible: true, localStatus: "Full local waveform ready.",
    } });
    destroy = () => unmount(instance);
    await tick();
    expect(target.querySelector('[aria-label="Analyze full local waveform"]')?.textContent).toBe("Full waveform ready");
    expect(target.querySelector('[role="status"]')?.textContent).toBe("Full local waveform ready.");
  });

  it("keeps the cancel action stable while showing progress separately", async () => {
    const target = document.createElement("div"); document.body.append(target);
    const onLocalCancel = vi.fn();
    const instance = mount(StreamMediaAnalysisSettings, { target, props: {
      enabled: false, onEnabledChange: vi.fn(), localEligible: true, localBusy: true,
      localStatus: "Analyzing local file · 38%", onLocalCancel,
    } });
    destroy = () => unmount(instance);
    await tick();
    const cancel = target.querySelector<HTMLButtonElement>('[aria-label="Cancel waveform analysis"]')!;
    expect(cancel.textContent).toBe("Cancel waveform analysis");
    expect(target.querySelector('[role="status"]')?.textContent).toContain("38%");
    cancel.click();
    expect(onLocalCancel).toHaveBeenCalledOnce();
  });
});
