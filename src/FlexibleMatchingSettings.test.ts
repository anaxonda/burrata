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
    expect(onchange).toHaveBeenCalledWith({version:1,allowOtherReleases:true,allowOtherPerformances:false,allowOtherMixes:true,allowOtherArtists:false,allowDifferentLengths:false});
    [...target.querySelectorAll("button")].find(button => button.textContent === "Got it")!.click();
    expect(onNoticeDismiss).toHaveBeenCalledOnce();
    unmount(component); target.remove();
  });
});
