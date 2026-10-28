// @vitest-environment jsdom
// @ts-expect-error Exercise Svelte's browser runtime in jsdom.
import { mount, tick, unmount } from "../node_modules/svelte/src/index-client.js";
import { afterEach, describe, expect, it, vi } from "vitest";
// @ts-expect-error Svelte internal client runtime has no public declarations.
vi.mock("svelte", async () => await import("../node_modules/svelte/src/index-client.js"));
import StreamAnalysisAction from "./StreamAnalysisAction.svelte";

let destroy: (() => void) | undefined;
afterEach(() => { destroy?.(); destroy = undefined; document.body.innerHTML = ""; });

describe("StreamAnalysisAction", () => {
  it("omits the action for local or otherwise inapplicable playback", async () => {
    const target = document.createElement("div"); document.body.append(target);
    const instance = mount(StreamAnalysisAction, { target, props: {
      eligible: false, state: "idle", onStart: vi.fn(), onCancel: vi.fn(),
    } });
    destroy = () => unmount(instance);
    await tick();
    expect(target.querySelector("button")).toBeNull();
  });

  it("shows an icon-only, named stream action and exposes clear capture and preparation states", async () => {
    const target = document.createElement("div"); document.body.append(target);
    const onStart = vi.fn(), onCancel = vi.fn();
    let instance = mount(StreamAnalysisAction, { target, props: { eligible: true, onStart, onCancel } });
    destroy = () => unmount(instance);
    await tick();
    let button = target.querySelector<HTMLButtonElement>("button")!;
    expect(button.getAttribute("aria-label")).toBe("Analyze this stream for BPM and key");
    expect(button.title).toBe("Analyze stream BPM and key");
    expect(button.textContent?.trim()).toBe("");
    expect(button.querySelector("svg[aria-hidden='true']")).not.toBeNull();
    button.click();
    expect(onStart).toHaveBeenCalledOnce();

    unmount(instance);
    instance = mount(StreamAnalysisAction, { target, props: { eligible: true, state: "preparing", onStart, onCancel } });
    destroy = () => unmount(instance);
    await tick();
    button = target.querySelector<HTMLButtonElement>("button")!;
    expect(button.disabled).toBe(true);
    expect(button.getAttribute("aria-busy")).toBe("true");
    expect(button.getAttribute("aria-label")).toContain("Preparing");
    expect(button.textContent?.trim()).toBe("");

    unmount(instance);
    instance = mount(StreamAnalysisAction, { target, props: { eligible: true, state: "capturing", onStart, onCancel } });
    destroy = () => unmount(instance);
    await tick();
    button = target.querySelector<HTMLButtonElement>("button")!;
    expect(button.disabled).toBe(false);
    expect(button.getAttribute("aria-label")).toBe("Cancel whole-track BPM and key analysis");
    expect(button.textContent?.trim()).toBe("");
    button.click();
    expect(onCancel).toHaveBeenCalledOnce();
  });
});
