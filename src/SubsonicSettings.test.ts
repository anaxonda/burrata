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

function render(status: SubsonicStatus | null, response: SubsonicStatus, provider?: "subsonic" | "jellyfin") {
  const target = document.createElement("div");
  document.body.append(target);
  const call = vi.fn(async () => response);
  const onStatus = vi.fn();
  instance = mount(SubsonicSettings, { target, props: { call, status, onStatus, provider } as never });
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

it("connects Jellyfin without overriding backend visibility or fallback defaults", async () => {
  const { target, call, onStatus } = render(null, connected, "jellyfin");
  expect(target.querySelector("h3")?.textContent).toBe("Jellyfin");
  const values = ["https://music.example.com", "listener", "fixture-password"];
  target.querySelectorAll<HTMLInputElement>("form input").forEach((input, index) => {
    input.value = values[index];
    input.dispatchEvent(new Event("input", { bubbles: true }));
  });
  target.querySelector("form")!.dispatchEvent(new SubmitEvent("submit", { bubbles: true, cancelable: true }));
  await tick();
  expect(call.mock.calls).toEqual([["connect_jellyfin", { endpoint: values[0], username: values[1], password: values[2] }]]);
  expect(onStatus).toHaveBeenCalledWith(connected);
  expect(target.querySelector<HTMLInputElement>('input[type="password"]')?.value).toBe("");
});

it("routes Jellyfin catalog controls independently and omits Strawberry migration", async () => {
  const current = { ...connected, canSeed: true, strawberryFound: true };
  const { target, call, onStatus } = render(current, current, "jellyfin");
  expect(target.textContent).toContain("Show Jellyfin in Library");
  expect(target.textContent).not.toContain("Strawberry");
  target.querySelectorAll<HTMLInputElement>('input[type="checkbox"]')[1].click();
  const button = (label: string) => [...target.querySelectorAll("button")].find(node => node.textContent?.trim() === label)!;
  await vi.waitFor(() => expect(onStatus).toHaveBeenCalledTimes(1));
  await tick();
  button("Refresh catalog").click();
  await vi.waitFor(() => expect(onStatus).toHaveBeenCalledTimes(2));
  await tick();
  button("Disconnect").click();
  await tick();
  expect(call.mock.calls).toEqual([
    ["set_jellyfin_preferences", { visible: true, allowFallback: true }],
    ["refresh_jellyfin", undefined],
    ["disconnect_jellyfin", undefined],
  ]);
});
