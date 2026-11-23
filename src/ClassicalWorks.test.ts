// @vitest-environment jsdom
// @ts-expect-error Exercise Svelte's browser runtime in jsdom.
import { mount, tick, unmount } from "../node_modules/svelte/src/index-client.js";
import { afterEach, expect, it, vi } from "vitest";
// @ts-expect-error Svelte internal client runtime lacks public declarations
vi.mock("svelte", async () => await import("../node_modules/svelte/src/index-client.js"));
import ClassicalWorks from "./ClassicalWorks.svelte";
import { classicalWorkPlacement } from "./lib/classical-works";
import type { Track } from "./lib/types";

const tracks: Track[] = [1, 2].map(part => ({
  id: `local:${part}`, provider: "local", providerId: String(part), sourceUrl: `local:${part}`, artist: "Pianist",
  title: `Movement ${part}`, album: "Piano sonatas", localPath: `/music/sonatas/${part}.flac`, trackNumber: part, favorite: false,
  provenance: {
    source: "local file tags",
    embeddedClassical: { version: 1, work: "Sonata, Op. 2", movement: `Movement ${part}`, movementNumber: part, movementTotal: 2, composers: ["Beethoven"] },
  },
}));
let instance: ReturnType<typeof mount> | undefined;
afterEach(async () => {
  if (instance) await unmount(instance);
  instance = undefined;
  document.body.innerHTML = "";
});

async function render(workKey: string | null, query = "") {
  const call = vi.fn(async () => ({ tracks: [...tracks].reverse(), total: tracks.length }));
  const onOpen = vi.fn(), onQuery = vi.fn(), onPlay = vi.fn();
  const target = document.createElement("div"); document.body.append(target);
  instance = mount(ClassicalWorks, { target, props: { call, query, workKey, onOpen, onQuery, trackActions: { onPlay } } as never });
  await tick();
  await Promise.resolve();
  await tick();
  return { target, call, onOpen, onQuery, onPlay };
}

it("filters work results by composer and opens that stable work key", async () => {
  const { target, call, onOpen, onQuery } = await render(null, "Beethoven 2");
  expect(call).toHaveBeenCalledWith("list_tracks_page", { view: "works", limit: 200, offset: 0 });
  const input = target.querySelector<HTMLInputElement>('input[type="search"]')!;
  expect(input.value).toBe("Beethoven 2");
  input.value = "Sonata";
  input.dispatchEvent(new Event("input", { bubbles: true }));
  expect(onQuery).toHaveBeenCalledWith("Sonata");
  target.querySelector<HTMLButtonElement>(".work-results button")!.click();
  expect(onOpen).toHaveBeenCalledWith(classicalWorkPlacement(tracks[0]!)?.key);
});

it("restores the saved album order and plays one movement or the entire tagged programme", async () => {
  const { target, onPlay } = await render(classicalWorkPlacement(tracks[0]!)!.key);
  target.querySelector<HTMLButtonElement>('[aria-label="Play work: Sonata, Op. 2"]')!.click();
  await tick();
  expect(onPlay).toHaveBeenCalledWith(tracks[0], tracks, 0);
  const rows = [...target.querySelectorAll<HTMLButtonElement>(".track-title")];
  rows[1]!.click();
  await tick();
  expect(onPlay).toHaveBeenLastCalledWith(tracks[1], [tracks[1]], 0);
});
