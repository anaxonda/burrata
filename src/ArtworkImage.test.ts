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
