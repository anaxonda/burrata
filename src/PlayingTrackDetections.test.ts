// @vitest-environment jsdom
// @ts-expect-error Exercise Svelte's browser runtime in jsdom.
import { mount, tick, unmount } from "../node_modules/svelte/src/index-client.js";
import { afterEach, describe, expect, it, vi } from "vitest";
// @ts-expect-error Match the browser runtime used by the direct import above.
vi.mock("svelte", async () => await import("../node_modules/svelte/src/index-client.js"));
import PlayingTrackDetections from "./PlayingTrackDetections.svelte";

let destroy: (() => void) | undefined;
afterEach(() => { destroy?.(); destroy = undefined; document.body.innerHTML = ""; });

function render(props: Record<string, unknown> = {}) {
  const target=document.createElement("div");document.body.append(target);
  const instance=mount(PlayingTrackDetections,{target,props:{onToggle:vi.fn(),onSeek:vi.fn(),...props} as never});
  destroy=()=>unmount(instance);
  return target;
}

describe("PlayingTrackDetections", () => {
  it("defaults the recording-level detector off and seeks to an approximate detected point", async () => {
    const onToggle=vi.fn(),onSeek=vi.fn();
    const target=render({onToggle,onSeek,observations:[{title:"Second song",artist:"Mix artist",detectedAtSeconds:125.8,capturedStartSeconds:120,capturedEndSeconds:132,observedAt:"2026-01-01T00:00:00Z"}]});
    await tick();
    const toggle=target.querySelector<HTMLInputElement>('input[type="checkbox"]')!;
    expect(toggle.checked).toBe(false);
    expect(target.textContent).toContain("separate from supplied chapters");
    expect(target.textContent).toContain("Detected at ~2:05");
    target.querySelector(".detected-track")!.dispatchEvent(new MouseEvent("click",{bubbles:true}));
    expect(onSeek).toHaveBeenCalledWith(125.8);
    toggle.checked=true;toggle.dispatchEvent(new Event("change",{bubbles:true}));
    expect(onToggle).toHaveBeenCalledWith(true);
  });

  it("keeps cached detections visible when detection is off and reports unsupported sources", async () => {
    const target=render({enabled:false,observations:[{title:"Saved result",artist:"Artist",detectedAtSeconds:4,capturedStartSeconds:0,capturedEndSeconds:8,observedAt:"2026-01-01T00:00:00Z"}]});
    await tick();
    expect(target.textContent).toContain("Saved result");
    expect(target.querySelector<HTMLInputElement>('input[type="checkbox"]')?.checked).toBe(false);
    destroy?.();destroy=undefined;
    document.body.innerHTML="";
    const unsupported=render({disabledReason:"Track detection needs a prepared audio source with a stable identity."});
    await tick();
    expect(unsupported.querySelector('input[type="checkbox"]')).toBeNull();
    expect(unsupported.textContent).toContain("stable identity");
  });
});
