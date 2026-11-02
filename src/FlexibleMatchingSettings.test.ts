// @vitest-environment jsdom
// @ts-expect-error Svelte browser runtime
import { mount, tick, unmount } from "../node_modules/svelte/src/index-client.js";
import { describe, it, expect, vi } from "vitest";
// @ts-expect-error Svelte internal runtime
vi.mock("svelte", async () => await import("../node_modules/svelte/src/index-client.js"));
import FlexibleMatchingSettings from "./FlexibleMatchingSettings.svelte";

describe("FlexibleMatchingSettings", () => {
  it("defaults to same recording releases and preserves other choices on change", async () => {
    const target = document.createElement("div"), onchange = vi.fn(), onNoticeDismiss = vi.fn();
    document.body.append(target);
    const component = mount(FlexibleMatchingSettings, {target, props:{onchange, onNoticeDismiss, noticePending:true}}); await tick();
    const boxes = [...target.querySelectorAll<HTMLInputElement>('input[type="checkbox"]')];
    expect(boxes.map(box => box.checked)).toEqual([true,false,false,false,false]);
    boxes[2].click();
    expect(onchange).toHaveBeenCalledWith({version:1,allowOtherReleases:true,allowOtherPerformances:false,allowOtherMixes:true,allowOtherArtists:false,allowDifferentLengths:false,maxShorterBySeconds:30,maxLongerBySeconds:30});
    [...target.querySelectorAll("button")].find(button => button.textContent === "Got it")!.click();
    expect(onNoticeDismiss).toHaveBeenCalledOnce();
    unmount(component); target.remove();
  });

  it("shows bounded separate duration limits and preserves them when toggled off", async () => {
    const target = document.createElement("div"), onchange = vi.fn();
    document.body.append(target);
    const component = mount(FlexibleMatchingSettings, {target, props:{onchange,value:{version:1,allowOtherReleases:true,allowOtherPerformances:false,allowOtherMixes:false,allowOtherArtists:false,allowDifferentLengths:true}}}); await tick();
    const shorter = target.querySelector<HTMLInputElement>('[aria-label="Maximum shorter duration difference (seconds)"]')!;
    const longer = target.querySelector<HTMLInputElement>('[aria-label="Maximum longer duration difference (seconds)"]')!;
    expect(shorter.value).toBe("30");
    expect(longer.value).toBe("30");
    expect(shorter.disabled).toBe(false);
    expect(longer.disabled).toBe(false);

    shorter.value = "75";
    shorter.dispatchEvent(new Event("change", {bubbles:true})); await tick();
    expect(onchange).toHaveBeenLastCalledWith(expect.objectContaining({allowDifferentLengths:true,maxShorterBySeconds:75,maxLongerBySeconds:30}));
    unmount(component); target.replaceChildren();

    const persistedChange = vi.fn();
    const remounted = mount(FlexibleMatchingSettings, {target, props:{onchange:persistedChange,value:{version:1,allowOtherReleases:true,allowOtherPerformances:false,allowOtherMixes:false,allowOtherArtists:false,allowDifferentLengths:true,maxShorterBySeconds:75,maxLongerBySeconds:30}}}); await tick();
    const allowDifferent = [...target.querySelectorAll<HTMLInputElement>('input[type="checkbox"]')][4];
    allowDifferent.click(); await tick();
    expect(persistedChange).toHaveBeenLastCalledWith(expect.objectContaining({allowDifferentLengths:false,maxShorterBySeconds:75,maxLongerBySeconds:30}));

    unmount(remounted); target.remove();
  });

  it("clamps entered limits to the supported seconds range", async () => {
    const target = document.createElement("div"), onchange = vi.fn();
    document.body.append(target);
    const component = mount(FlexibleMatchingSettings, {target, props:{onchange, value:{version:1,allowOtherReleases:true,allowOtherPerformances:false,allowOtherMixes:false,allowOtherArtists:false,allowDifferentLengths:true} as any}}); await tick();
    const shorter = target.querySelector<HTMLInputElement>('[aria-label="Maximum shorter duration difference (seconds)"]')!;
    shorter.value = "9999";
    shorter.dispatchEvent(new Event("change", {bubbles:true})); await tick();
    expect(onchange).toHaveBeenLastCalledWith(expect.objectContaining({maxShorterBySeconds:3600,maxLongerBySeconds:30}));
    unmount(component); target.remove();
  });
});
