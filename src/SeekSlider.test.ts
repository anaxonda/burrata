// @vitest-environment jsdom
// @ts-expect-error Use Svelte's browser runtime.
import { mount, unmount, tick } from "../node_modules/svelte/src/index-client.js";
import { afterEach, expect, it, vi } from "vitest";
import SeekSlider from "./SeekSlider.svelte";
// @ts-expect-error Use Svelte browser runtime for lifecycle hooks.
vi.mock("svelte", async () => await import("../node_modules/svelte/src/index-client.js"));
let instance: ReturnType<typeof mount>;
afterEach(async () => { if (instance) await unmount(instance); document.body.innerHTML = ""; vi.useRealTimers(); });
function render() {
  const onSeek = vi.fn();
  instance = mount(SeekSlider, {target:document.body,props:{position:0,duration:100,disabled:false,onSeek}});
  const input = document.querySelector("input")!;
  input.setPointerCapture = vi.fn();
  return {input,onSeek};
}
it("commits only the final pointer target", async () => {
  const {input,onSeek} = render();
  input.dispatchEvent(new Event("pointerdown", {bubbles:true}));
  for (const value of [10,20,30]) { input.value=String(value); input.dispatchEvent(new Event("input", {bubbles:true})); await tick(); }
  expect(onSeek).not.toHaveBeenCalled();
  input.dispatchEvent(new Event("pointerup", {bubbles:true}));
  expect(onSeek).toHaveBeenCalledExactlyOnceWith(30);
});
it("commits the current drag target if pointer capture is unexpectedly lost", async () => {
  const {input,onSeek} = render();
  input.dispatchEvent(new Event("pointerdown", {bubbles:true}));
  input.value = "42";
  input.dispatchEvent(new Event("input", {bubbles:true}));
  await tick();
  input.dispatchEvent(new Event("lostpointercapture", {bubbles:true}));
  expect(onSeek).toHaveBeenCalledExactlyOnceWith(42);
});
it("debounces keyboard scrubbing and cancels pending work on destruction", async () => {
  vi.useFakeTimers();
  const {input,onSeek} = render();
  for (const value of [10,20,30]) { input.value=String(value); input.dispatchEvent(new Event("input", {bubbles:true})); await tick(); vi.advanceTimersByTime(100); }
  expect(onSeek).not.toHaveBeenCalled();
  vi.advanceTimersByTime(50);
  expect(onSeek).toHaveBeenCalledExactlyOnceWith(30);
  input.value="40"; input.dispatchEvent(new Event("input", {bubbles:true}));
  await unmount(instance); instance=undefined as unknown as typeof instance;
  vi.advanceTimersByTime(200);
  expect(onSeek).toHaveBeenCalledTimes(1);
});
