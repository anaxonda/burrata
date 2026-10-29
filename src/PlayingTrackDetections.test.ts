// @vitest-environment jsdom
// @ts-expect-error Exercise Svelte's browser runtime in jsdom.
import { mount, tick, unmount } from "../node_modules/svelte/src/index-client.js";
import { afterEach, describe, expect, it, vi } from "vitest";
// @ts-expect-error Match the browser runtime used by the direct import above.
vi.mock("svelte", async () => await import("../node_modules/svelte/src/index-client.js"));
import PlayingTrackDetections from "./PlayingTrackDetections.svelte";
import { activeDetectedTrack } from "./lib/playing-track-detection";

let destroy: (() => void) | undefined;
afterEach(() => { destroy?.(); destroy = undefined; document.body.innerHTML = ""; });

function render(props: Record<string, unknown> = {}) {
  const target=document.createElement("div");document.body.append(target);
  const instance=mount(PlayingTrackDetections,{target,props:{onToggle:vi.fn(),onSeek:vi.fn(),...props} as never});
  destroy=()=>unmount(instance);
  return target;
}

describe("PlayingTrackDetections", () => {
  it("selects artwork for the latest detection at the current source position", () => {
    const items = [
      {title:"First",artist:"A",detectedAtSeconds:10,capturedStartSeconds:8,capturedEndSeconds:12,observedAt:"",artworkUrl:"https://example.test/first.jpg"},
      {title:"Second",artist:"B",detectedAtSeconds:30,capturedStartSeconds:28,capturedEndSeconds:32,observedAt:"",artworkUrl:"https://example.test/second.jpg"},
    ];
    expect(activeDetectedTrack(items, 29)?.artworkUrl).toBe(items[0]?.artworkUrl);
    expect(activeDetectedTrack(items, 30)?.title).toBe("Second");
    expect(activeDetectedTrack(items, 5)).toBeNull();
  });

  it("defaults the recording-level detector off and separates seeking from opening a detected track", async () => {
    const onToggle=vi.fn(),onSeek=vi.fn(),onOpenTrack=vi.fn();
    const observation={title:"Second song",artist:"Mix artist",detectedAtSeconds:125.8,capturedStartSeconds:120,capturedEndSeconds:132,observedAt:"2026-01-01T00:00:00Z"};
    const target=render({onToggle,onSeek,onOpenTrack,observations:[observation]});
    await tick();
    const toggle=target.querySelector<HTMLInputElement>('input[type="checkbox"]')!;
    expect(toggle.checked).toBe(false);
    expect(target.querySelector(".detection-heading")).toBeNull();
    const helpButton=target.querySelector<HTMLButtonElement>('[aria-label="About track detection"]')!;
    expect(helpButton).not.toBeNull();
    expect(helpButton.getAttribute("aria-expanded")).toBe("false");
    helpButton.click();
    await tick();
    expect(target.querySelector(".detection-help")?.textContent).toContain("short audio samples are sent to the recognition service");
    expect(target.textContent).toContain("~2:05");
    target.querySelector(".detection-time")!.dispatchEvent(new MouseEvent("click",{bubbles:true}));
    expect(onSeek).toHaveBeenCalledWith(125.8);
    expect(target.querySelector(".track-link")?.textContent).toBe("Second song");
    target.querySelector<HTMLButtonElement>(".track-link")!.click();
    expect(onOpenTrack).toHaveBeenCalledWith(observation);
    toggle.checked=true;toggle.dispatchEvent(new Event("change",{bubbles:true}));
    expect(onToggle).toHaveBeenCalledWith(true);
  });

  it("opens a detected track from its title or artist and supports adding one or all detections to playlists", async () => {
    const observations=[
      {title:"First",artist:"Artist A",detectedAtSeconds:4,capturedStartSeconds:0,capturedEndSeconds:8,observedAt:"2026-01-01T00:00:00Z"},
      {title:"Second",artist:"Artist B",detectedAtSeconds:14,capturedStartSeconds:10,capturedEndSeconds:18,observedAt:"2026-01-01T00:00:10Z"},
    ];
    const onOpenTrack=vi.fn(),onAddToPlaylist=vi.fn(),onAddAllToPlaylist=vi.fn();
    const target=render({observations,onOpenTrack,onAddToPlaylist,onAddAllToPlaylist});
    await tick();
    target.querySelector<HTMLButtonElement>(".track-link")!.click();
    target.querySelector<HTMLButtonElement>(".artist-link")!.click();
    target.querySelector<HTMLButtonElement>(".add-one")!.click();
    target.querySelector<HTMLButtonElement>(".add-all")!.click();
    expect(onOpenTrack).toHaveBeenNthCalledWith(1,observations[0]);
    expect(onOpenTrack).toHaveBeenNthCalledWith(2,observations[0]);
    expect(onAddToPlaylist).toHaveBeenCalledWith(observations[0]);
    expect(onAddAllToPlaylist).toHaveBeenCalledWith(observations);
  });

  it("keeps the opt-in control aligned and disables it when no supported source is available", async () => {
    const target=render({disabledReason:"Track detection needs a prepared audio source with a stable identity."});
    await tick();
    const row=target.querySelector(".detection-control-row")!;
    expect(row.querySelector("label")?.textContent).toContain("Detect tracks");
    expect(row.querySelector<HTMLInputElement>('input[type="checkbox"]')?.disabled).toBe(true);
    expect(row.querySelector('button[aria-label="About track detection"]')).not.toBeNull();
    expect(target.querySelector('[role="status"]')?.textContent).toContain("stable identity");
  });

  it("places marker controls below the detection and help row and forwards the preference", async () => {
    const onMarkersToggle=vi.fn();
    const target=render({markersEnabled:true,onMarkersToggle});
    await tick();
    const checkbox=target.querySelector<HTMLInputElement>('.detection-markers-toggle input')!;
    expect(checkbox.checked).toBe(true);
    const row=target.querySelector(".detection-control-row")!;
    expect(row.querySelector('.detection-help')).not.toBeNull();
    expect(row.querySelector('.detection-markers-toggle')).toBeNull();
    expect(row.nextElementSibling?.classList.contains('detection-markers-toggle')).toBe(true);
    expect(target.querySelector(".detection-markers-toggle")?.textContent).toContain("Show approximate song markers");
    checkbox.click();
    expect(onMarkersToggle).toHaveBeenCalledWith(false);
    destroy?.();destroy=undefined;
    const inactive=render({interactive:false,markersEnabled:true,onMarkersToggle});
    await tick();
    expect(inactive.querySelector(".detection-control-row")).toBeNull();
    expect(onMarkersToggle).toHaveBeenCalledTimes(1);
  });

  it("anchors help copy to the full control row so it stays within a narrow info panel", async () => {
    const target=render({disabledReason:"Track detection needs a prepared audio source with a stable identity."});
    await tick();
    const row=target.querySelector<HTMLElement>(".detection-control-row")!;
    const helpButton=target.querySelector<HTMLButtonElement>('[aria-label="About track detection"]')!;
    helpButton.click();
    await tick();
    const copy=target.querySelector<HTMLElement>(".detection-help-copy")!;
    expect(helpButton.getAttribute("aria-expanded")).toBe("true");
    expect(helpButton.getAttribute("aria-controls")).toBe(copy.id);
    expect(target.querySelector(".detection-help")?.parentElement).toBe(row);
    expect(copy.parentElement?.parentElement).toBe(row);
    expect(copy.getAttribute("role")).toBe("note");
    expect(copy.textContent).toContain("Detection is opt-in for this recording.");
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
    expect(unsupported.querySelector<HTMLInputElement>('input[type="checkbox"]')?.disabled).toBe(true);
    expect(unsupported.textContent).toContain("stable identity");
  });

  it("explains when saved detection history is full and preserved", async () => {
    const target=render({enabled:true,retentionLimitReached:true,observations:[{title:"Saved result",artist:"Artist",detectedAtSeconds:4,capturedStartSeconds:0,capturedEndSeconds:8,observedAt:"2026-01-01T00:00:00Z"}]});
    await tick();
    expect(target.querySelector('[role="status"]')?.textContent).toContain("existing timestamps were kept");
    expect(target.textContent).toContain("Saved result");
  });
});

it("dismisses detection help on Escape or outside interaction",async()=>{
 const target=render();await tick();
 const trigger=target.querySelector<HTMLButtonElement>('[aria-label="About track detection"]')!;
 trigger.click();await tick();
 trigger.dispatchEvent(new KeyboardEvent("keydown",{key:"Escape",bubbles:true,cancelable:true}));await tick();
 expect(trigger.getAttribute("aria-expanded")).toBe("false");
 expect(document.activeElement).toBe(trigger);
 trigger.click();await tick();
 document.body.dispatchEvent(new Event("pointerdown",{bubbles:true}));await tick();
 expect(trigger.getAttribute("aria-expanded")).toBe("false");
});
