// @vitest-environment jsdom
// @ts-expect-error Exercise Svelte's browser runtime in jsdom.
import { mount, tick, unmount } from "../node_modules/svelte/src/index-client.js";
import { afterEach, expect, it, vi } from "vitest";
// @ts-expect-error Match the browser runtime used by the direct import above.
vi.mock("svelte", async () => await import("../node_modules/svelte/src/index-client.js"));
import TrackDurationFilter from "./TrackDurationFilter.svelte";
let cleanup: (() => void) | undefined;
afterEach(() => {cleanup?.(); document.body.innerHTML = "";});

it("keeps active duration bounds when Filters closes and lets the user clear them explicitly", async () => {
  const onchange = vi.fn();
  const instance = mount(TrackDurationFilter, {target: document.body, props: {onchange}});
  cleanup = () => {void unmount(instance);};
  await tick();
  const toggle = document.querySelector<HTMLButtonElement>(".filter-toggle")!;
  const panel = document.querySelector<HTMLDivElement>(".duration-inputs")!;
  expect(toggle.getAttribute("aria-expanded")).toBe("false");
  expect(panel.hidden).toBe(true);
  toggle.click(); await tick();
  expect(toggle.getAttribute("aria-expanded")).toBe("true");
  const minimum = document.querySelector<HTMLInputElement>('[aria-label="Minimum track duration"]')!;
  minimum.value = "3:00"; minimum.dispatchEvent(new Event("input", {bubbles: true})); await tick();
  expect(document.querySelector('[aria-label="Duration filter active"]')).not.toBeNull();
  window.dispatchEvent(new KeyboardEvent("keydown", {key: "Escape"})); await tick();
  expect(panel.hidden).toBe(true);
  expect(document.activeElement).toBe(toggle);
  toggle.click(); await tick();
  expect(minimum.value).toBe("3:00");
  document.querySelector<HTMLButtonElement>(".clear-duration")!.click(); await tick();
  expect(minimum.value).toBe("");
  expect(document.querySelector('[aria-label="Duration filter active"]')).toBeNull();
  expect(onchange).toHaveBeenCalledTimes(2);
  expect(panel.hidden).toBe(false);
});

it.each([620, 100])("places Filters outside scrolling panes and keeps it above the player (trigger y=%s)", async (top) => {
  const section = document.createElement("section"); document.body.append(section);
  const footer = document.createElement("footer"); footer.className = "player"; document.body.append(footer);
  const box = (left: number, top: number, width: number, height: number) => ({left, top, width, height, right: left + width, bottom: top + height, x: left, y: top, toJSON: () => ({})});
  vi.spyOn(section, "getBoundingClientRect").mockReturnValue(box(450, 0, 300, 650));
  vi.spyOn(footer, "getBoundingClientRect").mockReturnValue(box(0, 650, 1024, 118));
  const instance = mount(TrackDurationFilter, {target: section}); cleanup = () => {void unmount(instance);};
  await tick();
  const root = section.querySelector<HTMLDivElement>(".duration-filter")!;
  const panel = section.querySelector<HTMLDivElement>(".duration-inputs")!;
  vi.spyOn(root, "getBoundingClientRect").mockReturnValue(box(500, top, 75, 34));
  vi.spyOn(panel, "getBoundingClientRect").mockReturnValue(box(0, 0, 220, 160));
  const toggle = root.querySelector<HTMLButtonElement>(".filter-toggle")!;
  toggle.click(); await tick(); await tick();
  expect(panel.parentElement).toBe(document.body);
  const renderedTop = Number.parseFloat(panel.style.top), renderedLeft = Number.parseFloat(panel.style.left);
  expect(renderedTop).toBeGreaterThanOrEqual(8);
  expect(renderedTop + 160).toBeLessThanOrEqual(642);
  expect(renderedLeft).toBeGreaterThanOrEqual(458);
  expect(renderedLeft + 220).toBeLessThanOrEqual(742);
  if (top === 620) expect(renderedTop + 160).toBeLessThan(top);
  else expect(renderedTop).toBeGreaterThan(top + 34);
  panel.querySelector("input")!.dispatchEvent(new Event("pointerdown", {bubbles: true})); await tick();
  panel.dispatchEvent(new Event("scroll")); await tick();
  expect(toggle.getAttribute("aria-expanded")).toBe("true");
  section.dispatchEvent(new Event("scroll")); await tick();
  expect(toggle.getAttribute("aria-expanded")).toBe("false");
  expect(panel.parentElement).toBe(root);
  toggle.click(); await tick(); await tick();
  window.dispatchEvent(new Event("resize")); await tick();
  expect(toggle.getAttribute("aria-expanded")).toBe("false");
});
