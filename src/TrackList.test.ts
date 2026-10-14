// @vitest-environment jsdom
// @ts-expect-error Exercise Svelte's browser runtime in jsdom.
import { mount, tick, unmount } from "../node_modules/svelte/src/index-client.js";
import { afterEach, describe, expect, it, vi } from "vitest";
// @ts-expect-error Match the browser runtime used by the direct import above.
vi.mock("svelte", async () => await import("../node_modules/svelte/src/index-client.js"));
import TrackList from "./TrackList.svelte";
import trackListSource from "./TrackList.svelte?raw";
import type { Track } from "./lib/types";

const tracks: Track[] = [
  { id: "track-1", provider: "library", providerId: "track-1", sourceUrl: "https://example.test/song", artist: "Artist Name", title: "Song Title", album: "Album", favorite: false, provenance: {} },
  { id: "track-2", provider: "library", providerId: "track-2", sourceUrl: "", artist: "Other Artist", title: "Other Song", favorite: true, provenance: {} },
];
let cleanup: (() => void) | undefined;
afterEach(() => { cleanup?.(); cleanup = undefined; document.body.innerHTML = ""; vi.restoreAllMocks(); });

function render(props: Record<string, unknown> = {}) {
  const target = document.createElement("div"); document.body.append(target);
  const instance = mount(TrackList, { target, props: { tracks, ...props } as never });
  cleanup = () => unmount(instance);
  return target;
}

describe("TrackList", () => {
  it("keeps title and artist separate, and uses compact track actions", () => {
    const onPlay = vi.fn(), onFavorite = vi.fn(), onDownload = vi.fn();
    const target = render({ onPlay, onFavorite, onDownload });
    const row = target.querySelector("li")!;
    expect(row.querySelector(".track-title")?.textContent?.trim()).toBe("Song Title");
    expect(row.querySelector(".track-subtitle")?.textContent).toContain("Artist Name");
    expect(row.querySelector(".track-subtitle")?.textContent).toContain("Album");
    row.querySelector<HTMLButtonElement>('[aria-label="Play Song Title"]')!.click();
    row.querySelector<HTMLButtonElement>('[aria-label="Add Song Title to favorites"]')!.click();
    row.querySelector<HTMLButtonElement>('[aria-label="Download Song Title"]')!.click();
    expect(onPlay).toHaveBeenCalledWith(tracks[0], tracks, 0);
    expect(onFavorite).toHaveBeenCalledWith(tracks[0]);
    expect(onDownload).toHaveBeenCalledWith(tracks[0]);
  });

  it("keeps standard queue actions in overflow and sends right-click to the full context menu", async () => {
    const onQueue = vi.fn(), onNext = vi.fn(), onContext = vi.fn();
    const target = render({ onQueue, onNext, onContext });
    const summary = target.querySelector<HTMLElement>('[aria-label="More actions for Song Title"]')!;
    summary.click(); await tick();
    const buttons = [...target.querySelectorAll<HTMLButtonElement>(".more-menu button")];
    buttons.find(button => button.textContent === "Play next")!.click();
    buttons.find(button => button.textContent === "Add to queue")!.click();
    buttons.find(button => button.textContent === "More actions…")!.click();
    expect(onNext).toHaveBeenCalledWith(tracks[0]);
    expect(onQueue).toHaveBeenCalledWith(tracks[0]);
    expect(onContext).toHaveBeenCalledWith(tracks[0], expect.any(MouseEvent));
    const contextEvent = new MouseEvent("contextmenu", { bubbles: true, cancelable: true });
    target.querySelector("li")!.dispatchEvent(contextEvent);
    expect(contextEvent.defaultPrevented).toBe(true);
    expect(onContext).toHaveBeenCalledTimes(2);
  });

  it("announces the current row and exposes buffering and selection states", () => {
    const onSelect = vi.fn();
    const target = render({ playingTrackId: "track-1", bufferingTrackId: "track-1", selectedTrackIds: ["track-1"], onSelect });
    const row = target.querySelector("li")!;
    expect(row.getAttribute("aria-current")).toBe("true");
    expect(row.classList.contains("buffering")).toBe(true);
    expect(row.classList.contains("selected")).toBe(true);
    expect(target.querySelector<HTMLInputElement>('[aria-label="Select Song Title"]')?.checked).toBe(true);
    const selectionEvent = new MouseEvent("click", { bubbles: true, shiftKey: true });
    target.querySelector<HTMLInputElement>('[aria-label="Select Song Title"]')!.dispatchEvent(selectionEvent);
    expect(onSelect).toHaveBeenCalledWith(tracks[0], selectionEvent);
  });

  it("includes reduced-motion handling for the buffering indicator", () => {
    const target = render({ bufferingTrackId: "track-1", onPlay: vi.fn() });
    expect(target.querySelector(".buffering .icon-action.play")).toBeTruthy();
    expect(trackListSource).toMatch(/prefers-reduced-motion\s*:\s*reduce/);
    expect(trackListSource).toMatch(/buffering[^\n]+animation\s*:\s*none/);
  });
});
