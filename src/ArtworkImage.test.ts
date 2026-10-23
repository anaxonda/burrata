// @vitest-environment jsdom
// @ts-expect-error Exercise Svelte's browser runtime in jsdom.
import { mount, tick, unmount } from "../node_modules/svelte/src/index-client.js";
import { afterEach, expect, it, vi } from "vitest";
// @ts-expect-error Svelte internal browser runtime.
vi.mock("svelte", async () => await import("../node_modules/svelte/src/index-client.js"));
import ArtworkImage from "./ArtworkImage.svelte";
let cleanup: (() => Promise<void>) | undefined;
afterEach(async () => { await cleanup?.(); document.body.innerHTML = ""; });

it("tries the alternate cover then replaces failed images with a placeholder", async () => {
  const instance = mount(ArtworkImage, {target:document.body,props:{urls:["https://coverartarchive.org/release/edition/front-250","https://coverartarchive.org/release-group/group/front-250"]}});
  cleanup = () => unmount(instance);
  await tick();
  expect(document.querySelector("img")?.src).toContain("/release/edition/");
  document.querySelector("img")!.dispatchEvent(new Event("error")); await tick();
  expect(document.querySelector("img")?.src).toContain("/release-group/group/");
  document.querySelector("img")!.dispatchEvent(new Event("error")); await tick();
  expect(document.querySelector("img")).toBeNull();
  expect(document.querySelector(".placeholder")?.textContent).toBe("♫");
});

it("shows a placeholder when no image exists", async () => {
  const instance = mount(ArtworkImage, {target:document.body,props:{urls:[]}});
  cleanup = () => unmount(instance); await tick();
  expect(document.querySelector("img")).toBeNull();
  expect(document.querySelector(".placeholder")).not.toBeNull();
});

it("upgrades a low-resolution hero only after it loads and keeps the original if the upgrade fails", async () => {
  const NativeImage = globalThis.Image;
  class ProbeImage {
    naturalWidth = 1200;
    naturalHeight = 1200;
    onload: ((event: Event) => void) | null = null;
    onerror: ((event: Event) => void) | null = null;
    set src(_value: string) { queueMicrotask(() => this.onerror?.(new Event("error"))); }
  }
  globalThis.Image = ProbeImage as unknown as typeof Image;
  const instance = mount(ArtworkImage, {target:document.body,props:{
    urls:["https://coverartarchive.org/release/edition/front-250"],
    upgradeCandidates:[{url:"https://coverartarchive.org/release/edition/front-1200",width:1200,height:1200}],
  }});
  cleanup = () => { globalThis.Image = NativeImage; return unmount(instance); };
  await tick();
  const original = document.querySelector<HTMLImageElement>("img")!;
  Object.defineProperty(original, "naturalWidth", { configurable: true, value: 250 });
  original.dispatchEvent(new Event("load"));
  await new Promise(resolve => setTimeout(resolve, 0)); await tick();
  expect(document.querySelector<HTMLImageElement>("img")?.src).toContain("front-250");
});

it("shows a successfully loaded high-resolution candidate", async () => {
  const NativeImage = globalThis.Image;
  class ProbeImage {
    naturalWidth = 1200;
    naturalHeight = 1200;
    onload: ((event: Event) => void) | null = null;
    onerror: ((event: Event) => void) | null = null;
    set src(_value: string) { queueMicrotask(() => this.onload?.(new Event("load"))); }
  }
  globalThis.Image = ProbeImage as unknown as typeof Image;
  const instance = mount(ArtworkImage, {target:document.body,props:{
    urls:["https://coverartarchive.org/release/edition/front-250"],
    upgradeCandidates:[{url:"https://coverartarchive.org/release/edition/front-1200",width:1200,height:1200}],
  }});
  cleanup = () => { globalThis.Image = NativeImage; return unmount(instance); };
  await tick();
  const original = document.querySelector<HTMLImageElement>("img")!;
  Object.defineProperty(original, "naturalWidth", { configurable: true, value: 250 });
  original.dispatchEvent(new Event("load"));
  await new Promise(resolve => setTimeout(resolve, 0)); await tick();
  expect(document.querySelector<HTMLImageElement>("img")?.src).toContain("front-1200");
});
