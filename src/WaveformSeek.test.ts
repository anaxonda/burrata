// @vitest-environment jsdom
// @ts-expect-error Exercise Svelte's browser runtime in jsdom.
import { mount, tick, unmount } from "../node_modules/svelte/src/index-client.js";
import { afterEach, describe, expect, it, vi } from "vitest";
import { createClassComponent } from "svelte/legacy";
// @ts-expect-error Svelte internal client runtime has no public declarations
vi.mock("svelte", async () => await import("../node_modules/svelte/src/index-client.js"));
import WaveformSeek from "./WaveformSeek.svelte";
import { emptyWaveform, localWaveformSnapshot, observeWaveformSegment } from "./lib/waveform-data";

let destroy: (() => void) | undefined;
afterEach(() => { destroy?.(); destroy = undefined; document.body.innerHTML = ""; vi.useRealTimers(); });
async function settle() { for (let i = 0; i < 8; i++) await Promise.resolve(); await tick(); }

describe("WaveformSeek", () => {
  it("renders observed bins with real gaps and an accessible seek range", async () => {
    let waveform = emptyWaveform(12, "song", 120, 12);
    waveform = observeWaveformSegment(waveform, { generation: 12, trackId: "song", startSeconds: 0, endSeconds: 10, min: -0.6, max: 0.5 });
    waveform = observeWaveformSegment(waveform, { generation: 12, trackId: "song", startSeconds: 70, endSeconds: 80, min: -0.4, max: 0.8 });
    const target = document.createElement("div"); document.body.append(target);
    const instance = mount(WaveformSeek, { target, props: { position: 50, duration: 120, disabled: false, onSeek: vi.fn(), waveform, generation: 12, trackId: "song" } as never });
    destroy = () => unmount(instance); await settle();
    const input = target.querySelector<HTMLInputElement>('[aria-label="Playback position, waveform"]');
    expect(input).not.toBeNull();
    expect(input?.title).toContain("Observed playback only");
    const status = target.querySelector(`#${input?.getAttribute("aria-describedby")}`);
    expect(status?.tagName).toBe("SPAN");
    expect(status?.classList.contains("sr-only")).toBe(true);
    expect(target.querySelectorAll("svg rect")).toHaveLength(2);
    expect(target.textContent).toContain("Observed playback only");
    expect(target.textContent).toContain("blank areas have not been analyzed");
    expect(target.querySelector(".waveform-unknown-rail")?.getAttribute("d")).toBeTruthy();
    expect(target.querySelector(".waveform-elapsed-rail")).not.toBeNull();
  });

  it("shows unknown timeline as a dotted rail without inventing waveform bars", async () => {
    let waveform = emptyWaveform(19, "long-mix", 7_200, 1_200);
    waveform = observeWaveformSegment(waveform, {
      generation: 19, trackId: "long-mix", startSeconds: 0, endSeconds: 300, min: -0.55, max: 0.6,
    });
    const target = document.createElement("div"); document.body.append(target);
    const instance = mount(WaveformSeek, { target, props: {
      position: 300, duration: 7_200, disabled: false, onSeek: vi.fn(), waveform, generation: 19, trackId: "long-mix",
    } as never });
    destroy = () => unmount(instance); await settle();

    expect(target.querySelectorAll(".waveform rect")).toHaveLength(50);
    expect(target.querySelector(".waveform-unknown-rail")?.getAttribute("d")).toContain("M50 91H1200");
    expect(target.querySelector(".waveform-elapsed-rail")?.getAttribute("x2")).toBe("50");
    const range = target.querySelector<HTMLInputElement>('[aria-label="Playback position, waveform"]')!;
    expect(range.min).toBe("0");
    expect(range.max).toBe("7200");
    expect(range.getAttribute("aria-describedby")).toBeTruthy();
  });

  it("commits keyboard/input changes at the unchanged seek position", async () => {
    vi.useFakeTimers();
    const onSeek = vi.fn(); const target = document.createElement("div"); document.body.append(target);
    const instance = mount(WaveformSeek, { target, props: { position: 7.5, duration: 90, disabled: false, onSeek, waveform: null, generation: 2, trackId: "song" } as never });
    destroy = () => unmount(instance); await settle();
    const input = target.querySelector<HTMLInputElement>('[aria-label="Playback position"]')!;
    input.value = "23.4"; input.dispatchEvent(new Event("input", { bubbles: true })); await settle();
    vi.advanceTimersByTime(151); await settle();
    expect(onSeek).toHaveBeenCalledOnce();
    expect(onSeek).toHaveBeenCalledWith(23.4);
  });

  it("previews and seeks an unobserved part of the full timeline without seeking on hover", async () => {
    vi.useFakeTimers();
    const onSeek = vi.fn();
    let waveform = emptyWaveform(19, "long-mix", 7_200, 1_200);
    waveform = observeWaveformSegment(waveform, {
      generation: 19, trackId: "long-mix", startSeconds: 0, endSeconds: 300, min: -0.5, max: 0.5,
    });
    const target = document.createElement("div"); document.body.append(target);
    const instance = mount(WaveformSeek, { target, props: {
      position: 300, duration: 7_200, disabled: false, onSeek, waveform, generation: 19, trackId: "long-mix",
    } as never });
    destroy = () => unmount(instance); await settle();
    const range = target.querySelector<HTMLInputElement>('[aria-label="Playback position, waveform"]')!;
    vi.spyOn(range, "getBoundingClientRect").mockReturnValue({ left: 10, width: 200 } as DOMRect);
    range.dispatchEvent(new MouseEvent("pointermove", { bubbles: true, clientX: 160 }));
    await settle();
    expect(target.querySelector(".waveform-hover-time")?.textContent).toBe("1:30:00");
    expect(range.title).toContain("Click or drag anywhere");
    expect(onSeek).not.toHaveBeenCalled();
    range.value = "5400";
    range.dispatchEvent(new Event("input", { bubbles: true }));
    vi.advanceTimersByTime(151); await settle();
    expect(onSeek).toHaveBeenCalledExactlyOnceWith(5400);
  });

  it("hides stale waveform generations or durations instead of drawing them for another track", async () => {
    const waveform = emptyWaveform(2, "old-song", 100, 10);
    const target = document.createElement("div"); document.body.append(target);
    const instance = mount(WaveformSeek, { target, props: { position: 8, duration: 100, disabled: false, onSeek: vi.fn(), waveform, generation: 3, trackId: "new-song" } as never });
    destroy = () => unmount(instance); await settle();
    expect(target.querySelectorAll("svg rect")).toHaveLength(0);
    const input = target.querySelector<HTMLInputElement>('[aria-label="Playback position"]');
    expect(input).not.toBeNull();
    expect(input?.value).toBe("8");
    expect(input?.max).toBe("100");
    expect(target.textContent).toContain("Standard seek bar shown");
  });

  it("uses the standard seek slider while a matching snapshot has only null bins", async () => {
    const waveform = emptyWaveform(4, "pending-song", 45, 16);
    const target = document.createElement("div"); document.body.append(target);
    const instance = mount(WaveformSeek, { target, props: {
      position: 12.5, duration: 45, disabled: false, onSeek: vi.fn(), waveform, generation: 4, trackId: "pending-song",
    } as never });
    destroy = () => unmount(instance); await settle();
    const input = target.querySelector<HTMLInputElement>('[aria-label="Playback position"]');
    expect(input).not.toBeNull();
    expect(input?.value).toBe("12.5");
    expect(input?.max).toBe("45");
    expect(target.querySelector(".waveform")).toBeNull();
    expect(target.querySelector(".waveform-status")?.textContent).toContain("Standard seek bar shown");
  });

  it("switches from the seek slider to waveform bars when valid samples arrive", async () => {
    const pending = emptyWaveform(8, "arriving-song", 20, 4);
    const target = document.createElement("div"); document.body.append(target);
    const instance = createClassComponent({ component: WaveformSeek, target, props: {
      position: 3, duration: 20, disabled: false, onSeek: vi.fn(), waveform: pending, generation: 8, trackId: "arriving-song",
    } as never }) as unknown as { $set: (props: { waveform: typeof pending }) => void };
    destroy = () => unmount(instance as never); await settle();
    expect(target.querySelector('[aria-label="Playback position"]')).not.toBeNull();
    expect(target.querySelector(".waveform")).toBeNull();

    const populated = observeWaveformSegment(pending, {
      generation: 8, trackId: "arriving-song", startSeconds: 0, endSeconds: 5, min: -0.3, max: 0.4,
    });
    instance.$set({ waveform: populated });
    await settle();
    expect(target.querySelector('[aria-label="Playback position, waveform"]')).not.toBeNull();
    expect(target.querySelector('[aria-label="Playback position"]')).toBeNull();
    expect(target.querySelectorAll(".waveform rect")).toHaveLength(1);
  });

  it("shows a disabled timeline label when duration is unknown or invalid", async () => {
    for (const duration of [Number.NaN, 0]) {
      document.body.innerHTML = "";
      const target = document.createElement("div"); document.body.append(target);
      const instance = mount(WaveformSeek, { target, props: {
        position: 9, duration, disabled: false, onSeek: vi.fn(), waveform: null, generation: 2, trackId: "song",
      } as never });
      await settle();
      const range = target.querySelector<HTMLInputElement>('[aria-label="Playback position unavailable"]');
      expect(range?.disabled).toBe(true);
      expect(range?.getAttribute("aria-valuetext")).toBe("Duration unavailable");
      expect(target.querySelector(".waveform-unavailable-label")?.textContent).toBe("Timeline unavailable");
      expect(target.querySelector(".waveform")).toBeNull();
      expect(target.querySelector("[role=status]")?.textContent).toContain("duration is unknown");
      await unmount(instance);
    }
  });

  it("makes quiet analyzed files easier to see without filling unmeasured bins", async () => {
    const waveform = localWaveformSnapshot({
      generation: 7, trackId: "quiet-song", durationSeconds: 4,
      bins: [null, { min: -0.02, max: 0.01 }, null, { min: -0.01, max: 0.02 }],
      complete: false,
    });
    const target = document.createElement("div"); document.body.append(target);
    const instance = mount(WaveformSeek, { target, props: {
      position: 1, duration: 4, disabled: false, onSeek: vi.fn(), waveform, generation: 7, trackId: "quiet-song",
    } as never });
    destroy = () => unmount(instance); await settle();

    const bars = [...target.querySelectorAll<SVGRectElement>("svg rect")];
    expect(bars).toHaveLength(2);
    expect(Number(bars[0]?.getAttribute("height"))).toBeGreaterThan(10);
    expect(Number(bars[1]?.getAttribute("height"))).toBeGreaterThan(10);
  });
});
