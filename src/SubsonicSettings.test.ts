// @vitest-environment jsdom
// @ts-expect-error Exercise Svelte's browser runtime in jsdom.
import { mount, tick, unmount } from "../node_modules/svelte/src/index-client.js";
import { afterEach, expect, it, vi } from "vitest";
// @ts-expect-error Svelte internal client runtime lacks public declarations.
vi.mock("svelte", async () => await import("../node_modules/svelte/src/index-client.js"));
import SubsonicSettings from "./SubsonicSettings.svelte";
import type { SubsonicStatus } from "./lib/subsonic";

const connected: SubsonicStatus = {
  endpoint: "https://music.example.com",
  username: "listener",
  visible: true,
  allowFallback: false,
  trackCount: 100,
  canSeed: false,
  strawberryFound: false,
};

let instance: ReturnType<typeof mount> | undefined;
afterEach(async () => {
  if (instance) await unmount(instance);
  instance = undefined;
  document.body.innerHTML = "";
});

function render(status: SubsonicStatus, response: SubsonicStatus) {
  const target = document.createElement("div");
  document.body.append(target);
  const call = vi.fn(async () => response);
  const onStatus = vi.fn();
  instance = mount(SubsonicSettings, { target, props: { call, status, onStatus } as never });
  return { target, call, onStatus };
}

it("hides the catalog through a visibility preference and keeps the saved-track fallback setting", async () => {
  const current = { ...connected, allowFallback: true };
  const hidden = { ...current, visible: false };
  const { target, call, onStatus } = render(current, hidden);
  target.querySelector<HTMLInputElement>('input[type="checkbox"]')!.click();
  await tick();
  expect(call.mock.calls).toEqual([["set_subsonic_preferences", { visible: false, allowFallback: true }]]);
  expect(onStatus).toHaveBeenCalledWith(hidden);
});

it("offers discovered and selected read-only Strawberry seeds before a catalog is created", async () => {
  const current = { ...connected, canSeed: true, strawberryFound: true, trackCount: 0 };
  const { target, call, onStatus } = render(current, connected);
  [...target.querySelectorAll("button")].find(button => button.textContent?.includes("Use Strawberry cache"))!.click();
  await tick();
  expect(call.mock.calls).toEqual([["seed_subsonic", { discovered: true }]]);
  expect(onStatus).toHaveBeenCalledWith(connected);
});
