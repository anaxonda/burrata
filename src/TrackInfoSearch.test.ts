// @vitest-environment jsdom
// @ts-expect-error Use Svelte's browser runtime because Vitest otherwise picks its server export.
import { mount, tick, unmount } from "../node_modules/svelte/src/index-client.js";
import { afterEach, describe, expect, it, vi } from "vitest";
// @ts-expect-error Match the browser runtime used by the direct import above.
vi.mock("svelte", async () => await import("../node_modules/svelte/src/index-client.js"));
import TrackInfoSearch from "./TrackInfoSearch.svelte";
import type { Track, Candidate } from "./lib/types";

const track: Track = {
  id: "chapter-instance", provider: "chapter", providerId: "parent:chapter-1", title: "Quiet Tide",
  artist: "Aster Vale", favorite: false, provenance: {}, sourceUrl: "https://example.test/source",
  playbackRange: { parentTrackId: "parent", chapterId: "chapter-1", sourceUrl: "https://example.test/source", startTime: 40, endTime: 130, parentTitle: "Long Mix" },
};
const candidate = { id: "audio-source", provider: "youtube", url: "https://youtube.com/watch?v=abc", title: "Long Mix", uploader: "Source Creator" } as Candidate;
let cleanup: (() => void) | undefined;
afterEach(() => { cleanup?.(); cleanup = undefined; document.body.innerHTML = ""; });

describe("TrackInfoSearch", () => {
  it("seeds chapter context, permits editing, and routes searches without metadata changes", async () => {
    const onCatalogSearch = vi.fn(), onWebSearch = vi.fn(), onOpenSource = vi.fn(), onClose = vi.fn();
    const instance = mount(TrackInfoSearch, { target: document.body, props: { track, candidate, chapterTitle: "Quiet Tide", onCatalogSearch, onWebSearch, onOpenSource, onClose } });
    cleanup = () => { void unmount(instance); };
    await tick();
    const dialog = document.querySelector<HTMLDialogElement>("dialog.info-search")!;
    expect(dialog.textContent).toContain("Chapter: Quiet Tide");
    expect(dialog.textContent).toContain("from Long Mix");
    expect(dialog.textContent).toContain("Source Creator");
    const input = dialog.querySelector<HTMLInputElement>("input")!;
    expect(input.value).toContain("Quiet Tide");
    input.value = "Aster Vale Quiet Tide extended mix";
    input.dispatchEvent(new Event("input", { bubbles: true }));
    dialog.querySelector<HTMLButtonElement>('button[type="submit"]')!.click();
    expect(onCatalogSearch).toHaveBeenCalledWith("Aster Vale Quiet Tide extended mix");
    dialog.querySelector<HTMLButtonElement>(".source")!.click();
    expect(onOpenSource).toHaveBeenCalledWith(candidate.url);
  });
});
