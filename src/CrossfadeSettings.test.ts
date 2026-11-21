// @vitest-environment jsdom
// @ts-expect-error Svelte browser runtime
import { mount, tick, unmount } from "../node_modules/svelte/src/index-client.js";
import { describe, it, expect, vi } from "vitest";
// @ts-expect-error Svelte internal runtime
vi.mock("svelte", async () => await import("../node_modules/svelte/src/index-client.js"));
import CrossfadeSettings from "./CrossfadeSettings.svelte";

describe("CrossfadeSettings", () => {
  it("defaults off and retains the five second duration when enabled", async () => {
    const target = document.createElement("div"), onchange = vi.fn();
    document.body.append(target);
    const component = mount(CrossfadeSettings, {target, props:{onchange}}); await tick();
    expect(target.querySelector<HTMLInputElement>('input')!.checked).toBe(false);
    target.querySelector<HTMLInputElement>('input')!.click();
    expect(onchange).toHaveBeenCalledWith(true, 5);
    unmount(component); target.remove();
  });
  it("offers whole-second durations and reports selection", async () => {
    const target = document.createElement("div"), onchange = vi.fn();
    document.body.append(target);
    const component = mount(CrossfadeSettings, {target, props:{enabled:true,seconds:7,onchange}}); await tick();
    const select = target.querySelector<HTMLSelectElement>('select')!;
    expect([...select.options].map(option => Number(option.value))).toEqual([1,2,3,4,5,6,7,8,9,10,11,12]);
    expect(select.value).toBe("7"); select.value = "12"; select.dispatchEvent(new Event("change", {bubbles:true}));
    expect(onchange).toHaveBeenCalledWith(true,12); unmount(component); target.remove();
  });
  it("offers Soft Next independently of natural crossfade", async () => {
    const target = document.createElement("div");
    document.body.append(target);
    const onsoftnextchange = vi.fn();
    const component = mount(CrossfadeSettings, {
      target,
      props: { softNext: true, onchange: vi.fn(), onsoftnextchange },
    });
    await tick();
    const [natural, softNext] = target.querySelectorAll<HTMLInputElement>('input[type="checkbox"]');
    expect(natural.checked).toBe(false);
    expect(softNext.checked).toBe(true);
    expect(target.querySelector("select")?.value).toBe("5");
    softNext.click();
    expect(onsoftnextchange).toHaveBeenCalledWith(false);
    unmount(component);
    target.remove();
  });

  it("offers track selection fade independently of Soft Next", async () => {
    const target = document.createElement("div");
    document.body.append(target);
    const onsofttrackselectionchange = vi.fn();
    const component = mount(CrossfadeSettings, {
      target,
      props: {
        softTrackSelection: true,
        onchange: vi.fn(),
        onsoftnextchange: vi.fn(),
        onsofttrackselectionchange,
      },
    });
    await tick();
    const [natural, softNext, trackSelection] = target.querySelectorAll<HTMLInputElement>('input[type="checkbox"]');
    expect([natural.checked, softNext.checked, trackSelection.checked]).toEqual([false, false, true]);
    expect(target.querySelector("select")?.value).toBe("5");
    trackSelection.click();
    expect(onsofttrackselectionchange).toHaveBeenCalledWith(false);
    unmount(component);
    target.remove();
  });
});
