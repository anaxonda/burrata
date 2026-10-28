// @vitest-environment jsdom
// @ts-expect-error Exercise Svelte's browser runtime in jsdom.
import { mount, tick, unmount } from "../node_modules/svelte/src/index-client.js";
import { afterEach, describe, expect, it, vi } from "vitest";
// @ts-expect-error Svelte internal client runtime has no public declarations.
vi.mock("svelte", async () => await import("../node_modules/svelte/src/index-client.js"));
import StreamMediaAnalysisAction from "./StreamMediaAnalysisAction.svelte";

let destroy: (() => void) | undefined;
afterEach(() => { destroy?.(); destroy = undefined; document.body.innerHTML = ""; });

describe("StreamMediaAnalysisAction", () => {
  it("stays hidden for local or ineligible sources", async () => {
    const target = document.createElement("div"); document.body.append(target);
    const instance = mount(StreamMediaAnalysisAction, { target, props: { eligible: false, onStart: vi.fn(), onCancel: vi.fn() } });
    destroy = () => unmount(instance);
    await tick();
    expect(target.querySelector("button")).toBeNull();
  });

  it("offers a compact accessible action and a cancelable running state", async () => {
    const target = document.createElement("div"); document.body.append(target);
    const onStart = vi.fn(), onCancel = vi.fn();
    let instance = mount(StreamMediaAnalysisAction, { target, props: { eligible: true, onStart, onCancel } });
    destroy = () => unmount(instance);
    await tick();
    let button = target.querySelector<HTMLButtonElement>("button")!;
    expect(button.getAttribute("aria-label")).toContain("BPM, key, and waveform");
    expect(button.textContent?.trim()).toBe("Analyze current stream");
    button.click();
    expect(onStart).toHaveBeenCalledOnce();

    unmount(instance);
    instance = mount(StreamMediaAnalysisAction, { target, props: {
      eligible: true, state: "running", status: "Analyzing verified stream · 1 of 1", onStart, onCancel,
    } });
    destroy = () => unmount(instance);
    await tick();
    button = target.querySelector<HTMLButtonElement>("button")!;
    expect(button.disabled).toBe(false);
    expect(button.title).toContain("1 of 1");
    button.click();
    expect(onCancel).toHaveBeenCalledOnce();
  });

  it("shows an explicit busy state while the resolver prepares", async () => {
    const target = document.createElement("div"); document.body.append(target);
    const instance = mount(StreamMediaAnalysisAction, { target, props: { eligible: true, state: "starting", onStart: vi.fn(), onCancel: vi.fn() } });
    destroy = () => unmount(instance);
    await tick();
    const button = target.querySelector<HTMLButtonElement>("button")!;
    expect(button.disabled).toBe(true);
    expect(button.getAttribute("aria-busy")).toBe("true");
    expect(button.getAttribute("aria-label")).toContain("faster-than-realtime");
  });
});
