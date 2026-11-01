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
});
