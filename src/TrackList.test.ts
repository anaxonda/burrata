// @vitest-environment jsdom
// @ts-expect-error Exercise Svelte's browser runtime in jsdom.
import { mount, tick, unmount } from "../node_modules/svelte/src/index-client.js";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
// @ts-expect-error Match the browser runtime used by the direct import above.
vi.mock("svelte", async () => await import("../node_modules/svelte/src/index-client.js"));
import TrackList from "./TrackList.svelte";
import { SOURCE_OPENER } from "./lib/source-opening";
// @ts-expect-error Reuse the reactive prop proxy for context-replacement coverage.
import { proxy } from "../node_modules/svelte/src/internal/client/proxy.js";
import trackListSource from "./TrackList.svelte?raw";
import bufferingSpinnerSource from "./BufferingSpinner.svelte?raw";
import type { Track } from "./lib/types";
import { playlistDropIndex } from "./lib/playlist-reorder";
import { measureTrackListCounts } from "./lib/playback-layout";

const tracks: Track[] = [
  { id: "track-1", provider: "library", providerId: "track-1", sourceUrl: "https://example.test/song", artist: "Artist Name", title: "Song Title", album: "Album", favorite: false, provenance: {} },
  { id: "track-2", provider: "library", providerId: "track-2", sourceUrl: "", artist: "Other Artist", title: "Other Song", favorite: true, provenance: {} },
];
let cleanup: (() => void) | undefined;
beforeEach(() => {
  globalThis.ResizeObserver = class { observe() {} unobserve() {} disconnect() {} } as unknown as typeof ResizeObserver;
  Object.defineProperty(HTMLDialogElement.prototype, "showModal", {configurable: true, value: function(this: HTMLDialogElement) {this.setAttribute("open", "");}});
});
afterEach(() => { cleanup?.(); cleanup = undefined; document.body.innerHTML = ""; vi.restoreAllMocks(); });

function resetRendered() { cleanup?.(); cleanup = undefined; document.body.innerHTML = ""; }

function render(props: Record<string, unknown> = {}) {
  const target = document.createElement("div"); document.body.append(target);
  const instance = mount(TrackList, { target, props: { tracks, ...props } as never });
  cleanup = () => unmount(instance);
  return target;
}

describe("TrackList", () => {
  it("reports 10k loaded rows with a virtual window and occurrence-specific visible controls", async () => {
    vi.stubGlobal("requestAnimationFrame", (callback: FrameRequestCallback) => { callback(0); return 1; });
    const rows: Track[] = Array.from({ length: 10_000 }, (_, index) => ({
      ...(index < 2 ? tracks[0]! : tracks[1]!),
      id: index < 2 ? "duplicate-track" : `track-${index}`,
      provenance: { playlistPosition: index + 1 },
    }));
    const trackArea = document.createElement("section"); trackArea.className = "track-area";
    const desktopList = document.createElement("div"); desktopList.className = "track-scroll desktop-track-list";
    trackArea.append(desktopList); document.body.append(trackArea);
    const instance = mount(TrackList, { target: desktopList, props: {
      tracks: rows,
      selectionMode: true,
      selectedTrackIds: ["occurrence-1"],
      selectionKeys: rows.map((_, index) => `occurrence-${index}`),
      rowKeys: rows.map((_, index) => `row-${index}`),
      reorderable: true,
      reorderCount: rows.length,
      onReorder: () => {},
      onSelect: () => {},
      virtualizationThreshold: 100,
    } as never });
    cleanup = () => unmount(instance);
    await tick();
    const logicalList = desktopList.querySelector("ol.shared-track-list")!;
    expect(logicalList.getAttribute("data-loaded-logical-rows")).toBe("10000");
    const renderedRows = [...logicalList.querySelectorAll("li[data-track-index]")];
    expect(renderedRows.length).toBeGreaterThan(2);
    expect(renderedRows.length).toBeLessThan(100);
    expect(renderedRows.slice(0, 2).map(row => row.getAttribute("data-track-index"))).toEqual(["0", "1"]);
    expect(renderedRows[0]?.querySelector<HTMLInputElement>("input.select-track")?.checked).toBe(false);
    expect(renderedRows[1]?.querySelector<HTMLInputElement>("input.select-track")?.checked).toBe(true);

    const viewport = desktopList.querySelector(".track-list-viewport")!;
    for (const element of desktopList.querySelectorAll("*")) {
      const row = element.closest("li[data-track-index]");
      const index = row ? Number(row.getAttribute("data-track-index")) : -1;
      const rowTop = index < 0 ? 0 : 10 + index * 24;
      const control = element.matches("input,button");
      const top = control ? rowTop + 3 : rowTop;
      const height = control ? 18 : 24;
      const isViewport = element === viewport;
      Object.defineProperties(element, {
        getBoundingClientRect: { value: () => ({ left: 0, top: isViewport ? 0 : top, right: 300, bottom: isViewport ? 200 : top + height, width: 300, height: isViewport ? 200 : height }) },
        getClientRects: { value: () => [{}] },
      });
    }
    Object.defineProperty(desktopList, "getBoundingClientRect", { value: () => ({ left: 0, top: 0, right: 300, bottom: 200, width: 300, height: 200 }) });
    const counts = measureTrackListCounts(trackArea, { width: 300, height: 200 }, element => ({
      display: "block", visibility: "visible", opacity: "1",
      overflowY: element === viewport ? "auto" : "visible",
    }));
    expect(counts.find(item => item.role === "library")).toMatchObject({
      loadedLogicalRows: 10_000,
      visibleRows: 8,
      visibleSelectedRows: 1,
      visibleCheckboxes: 8,
      visibleCheckedCheckboxes: 1,
      visibleReorderHandles: 8,
    });
    expect(JSON.stringify(counts)).not.toMatch(/duplicate-track|track-\d|Song Title|Other Song|https?:/i);
  });

  it("keeps the full logical count when App passes only its parent-virtualized slice", async () => {
    const allRows: Track[] = Array.from({ length: 1_200 }, (_, index) => ({
      ...tracks[index % tracks.length]!,
      id: `logical-track-${index}`,
    }));
    const parentWindow = allRows.slice(600, 620);
    const target = render({ tracks: parentWindow, logicalRowCount: allRows.length, virtualizationThreshold: 100_000 });
    await tick();
    const logicalList = target.querySelector("ol.shared-track-list")!;
    expect(logicalList.getAttribute("data-loaded-logical-rows")).toBe("1200");
    expect(logicalList.querySelectorAll("li[data-track-index]")).toHaveLength(20);
    expect(logicalList.classList.contains("virtualized")).toBe(false);
  });

  it("defaults logical rows to tracks length and clamps the observer metadata", async () => {
    const fallback = render({ tracks: [tracks[0]!, tracks[1]!] });
    await tick();
    expect(fallback.querySelector("ol.shared-track-list")?.getAttribute("data-loaded-logical-rows")).toBe("2");
    resetRendered();

    const clamped = render({ tracks: [tracks[0]!, tracks[1]!], logicalRowCount: 50_000 });
    await tick();
    expect(clamped.querySelector("ol.shared-track-list")?.getAttribute("data-loaded-logical-rows")).toBe("10000");
    resetRendered();

    const lowerBounded = render({ tracks: [tracks[0]!, tracks[1]!], logicalRowCount: 1 });
    await tick();
    expect(lowerBounded.querySelector("ol.shared-track-list")?.getAttribute("data-loaded-logical-rows")).toBe("2");
  });

  it("highlights and focuses one exact duplicate occurrence by stable row key", async () => {
    vi.stubGlobal("requestAnimationFrame", (callback: FrameRequestCallback) => { callback(0); return 1; });
    const repeated = { ...tracks[0]! };
    const rows = [tracks[0]!, repeated];
    const target = render({
      tracks: rows,
      orderedRows: [
        { kind: "track", key: "session-first", trackIndex: 0 },
        { kind: "track", key: "session-second", trackIndex: 1 },
      ],
      focusedRowKey: "session-second",
    });
    await tick();
    const highlighted = target.querySelectorAll("li.focused-history-occurrence");
    expect(highlighted).toHaveLength(1);
    expect(highlighted[0]?.getAttribute("data-row-key")).toBe("session-second");
    expect(highlighted[0]?.getAttribute("aria-current")).toBe("location");
    expect(document.activeElement).toBe(highlighted[0]?.querySelector("button.track-title"));
    expect(target.querySelectorAll('li[aria-current="location"]')).toHaveLength(1);
  });

  it("uses global row keys for a previrtualized slice with duplicate track IDs", async () => {
    vi.stubGlobal("requestAnimationFrame", (callback: FrameRequestCallback) => { callback(0); return 1; });
    const repeated = { ...tracks[0]! };
    const target = render({
      tracks: [repeated, { ...repeated }],
      rowKeys: ["track-1:999", "track-1:1000"],
      selectionKeys: ["selection-occurrence-0", "selection-occurrence-1"],
      focusedRowKey: "track-1:1000",
    });
    await tick();
    const rows = [...target.querySelectorAll<HTMLLIElement>("li[data-track-index]")];
    expect(rows.map(row => row.getAttribute("data-row-key"))).toEqual(["track-1:999", "track-1:1000"]);
    expect(rows.map(row => row.classList.contains("focused-history-occurrence"))).toEqual([false, true]);
    expect(rows[1]?.getAttribute("aria-current")).toBe("location");
    expect(document.activeElement).toBe(rows[1]?.querySelector("button.track-title"));
  });

  it("calculates final persisted indexes for before/after drops without identifying rows by track ID", () => {
    expect(playlistDropIndex(2, 4, true)).toBe(3);
    expect(playlistDropIndex(4, 2, false)).toBe(1);
    expect(playlistDropIndex(2, 4, false)).toBe(2);
    expect(playlistDropIndex(4, 2, true)).toBe(2);
    expect(playlistDropIndex(2, 2, true)).toBeNull();
    expect(playlistDropIndex(0, 2, false)).toBeNull();
  });

  it("reorders playlist occurrences by persisted position, including duplicate track IDs", async () => {
    const repeated = { ...tracks[0], provenance: { playlistPosition: 2 } };
    const rows: Track[] = [
      { ...tracks[0], provenance: { playlistPosition: 1 } },
      repeated,
      { ...tracks[1], provenance: { playlistPosition: 3 } },
    ];
    const onReorder = vi.fn();
    const target = render({ tracks: rows, presentation: "playlist", onPlay: vi.fn(), onAddSelectedToPlaylist: vi.fn(), reorderable: true, onReorder });
    expect(target.querySelector(".playlist-reorder-handle")).toBeNull();
    target.querySelector<HTMLButtonElement>(".start-selection")!.click(); await tick();
    const handle = target.querySelector<HTMLButtonElement>('[aria-label^="Reorder Song Title at position 2"]')!;
    const data = { setData: vi.fn(), effectAllowed: "", dropEffect: "" } as unknown as DataTransfer;
    const dragStart = new Event("dragstart", { bubbles: true, cancelable: true });
    Object.defineProperty(dragStart, "dataTransfer", { value: data });
    handle.dispatchEvent(dragStart);
    const dropRow = target.querySelector<HTMLElement>('li[data-track-index="2"]')!;
    vi.spyOn(dropRow, "getBoundingClientRect").mockReturnValue({ top: 0, height: 100, bottom: 100, left: 0, right: 100, width: 100, x: 0, y: 0, toJSON() {} });
    const dragOver = new Event("dragover", { bubbles: true, cancelable: true });
    Object.defineProperty(dragOver, "dataTransfer", { value: data });
    dropRow.dispatchEvent(dragOver);
    const drop = new Event("drop", { bubbles: true, cancelable: true });
    Object.defineProperty(drop, "dataTransfer", { value: data });
    Object.defineProperty(drop, "clientY", { value: 90 });
    dropRow.dispatchEvent(drop);
    await tick();
    expect(onReorder).toHaveBeenCalledWith(1, 2);
    handle.dispatchEvent(new Event("dragend", { bubbles: true }));
  });

  it("opens the same context callback directly from ellipsis and right-click without an intermediate menu", async () => {
    const onContext = vi.fn();
    const target = render({onContext, onQueue: vi.fn(), onPlaylist: vi.fn()});
    const trigger = target.querySelector<HTMLButtonElement>('.more-trigger')!;
    expect(trigger.getAttribute("aria-haspopup")).toBe("dialog");
    trigger.click(); await tick();
    expect(onContext).toHaveBeenCalledWith(tracks[0], expect.any(MouseEvent));
    expect(target.querySelector(".track-menu")).toBeNull();
    expect(target.textContent).not.toContain("More actions…");
    const event = new MouseEvent("contextmenu", {bubbles: true, cancelable: true});
    target.querySelector("li")!.dispatchEvent(event);
    expect(event.defaultPrevented).toBe(true);
    expect(onContext).toHaveBeenLastCalledWith(tracks[0], event);
    expect(onContext).toHaveBeenCalledTimes(2);
  });

  it("uses canonical standalone actions and only exposes supported callbacks", async () => {
    const onFavorite = vi.fn(), onDownload = vi.fn();
    const target = render({onFavorite, onDownload});
    const open = async () => {target.querySelector<HTMLButtonElement>('.more-trigger')!.click(); await tick();};
    await open();
    let dialog = target.querySelector<HTMLDialogElement>(".track-menu")!;
    expect(dialog.open).toBe(true);
    const labels = [...dialog.querySelectorAll<HTMLButtonElement>(":scope > button")].map(button => button.textContent?.trim());
    expect(labels).toContain("Add to Liked Songs");
    expect(labels).toContain("Download options…");
    expect(labels).not.toContain("Play next");
    expect(labels).not.toContain("Add to queue");
    expect(labels).not.toContain("Track information");
    expect(dialog.querySelector<HTMLButtonElement>('[aria-label^="Go to album"]')?.disabled).toBe(true);
    [...dialog.querySelectorAll<HTMLButtonElement>("button")].find(button => button.textContent?.trim() === "Add to Liked Songs")!.click(); await tick();
    expect(onFavorite).toHaveBeenCalledWith(tracks[0]);
    expect(target.querySelector(".track-menu")).toBeNull();
    await open(); dialog = target.querySelector<HTMLDialogElement>(".track-menu")!;
    [...dialog.querySelectorAll<HTMLButtonElement>("button")].find(button => button.textContent?.trim() === "Download options…")!.click(); await tick();
    expect(onDownload).toHaveBeenCalledWith(tracks[0]);
  });

  it("preserves standalone queue, artist, information and local batch selection capabilities", async () => {
    const onNext = vi.fn(), onQueue = vi.fn(), onPlaylist = vi.fn(), onArtist = vi.fn(), onInfo = vi.fn(), onAddSelectedToPlaylist = vi.fn();
    const target = render({onNext, onQueue, onPlaylist, onArtist, onInfo, onAddSelectedToPlaylist});
    for (const [label, callback] of [["Play next", onNext], ["Add to queue", onQueue], ["Add to playlist…", onPlaylist], ["Open artist", onArtist], ["Track information", onInfo]] as const) {
      target.querySelector<HTMLButtonElement>('.more-trigger')!.click(); await tick();
      [...target.querySelectorAll<HTMLButtonElement>(".track-menu button")].find(button => button.textContent?.trim() === label)!.click(); await tick();
      expect(callback).toHaveBeenCalledWith(tracks[0]);
    }
    target.querySelector<HTMLButtonElement>('.more-trigger')!.click(); await tick();
    [...target.querySelectorAll<HTMLButtonElement>(".track-menu button")].find(button => button.textContent?.trim() === "Select for batch actions")!.click(); await tick();
    expect(target.querySelector<HTMLInputElement>('[aria-label="Select Song Title for playlist"]')?.checked).toBe(true);
    target.querySelector<HTMLButtonElement>(".add-selected")!.click();
    expect(onAddSelectedToPlaylist).toHaveBeenCalledWith([tracks[0]]);
  });

  it("passes the exact row track to the canonical contextual source action and handles failures safely", async () => {
    const opener = vi.fn().mockResolvedValue(undefined);
    const target = document.createElement("div"); document.body.append(target);
    const instance = mount(TrackList, {target, props: {tracks}, context: new Map([[SOURCE_OPENER, opener]])}); cleanup = () => unmount(instance);
    target.querySelector<HTMLButtonElement>('.more-trigger')!.click(); await tick();
    target.querySelector<HTMLButtonElement>(".track-menu .source-action")!.click(); await tick();
    expect(opener).toHaveBeenCalledWith(tracks[0], tracks[0].sourceUrl);
    opener.mockRejectedValueOnce(new Error("private-source"));
    target.querySelector<HTMLButtonElement>('.more-trigger')!.click(); await tick();
    target.querySelector<HTMLButtonElement>(".track-menu .source-action")!.click(); await Promise.resolve(); await tick();
    expect(target.querySelector('[role="alert"]')?.textContent).toBe("Could not open the recording source.");
    expect(target.textContent).not.toContain("private-source");
  });

  it("keeps title and artist separate, and uses compact track actions", () => {
    const onPlay = vi.fn(), onFavorite = vi.fn(), onDownload = vi.fn();
    const target = render({onPlay, onFavorite, onDownload});
    const row = target.querySelector("li")!;
    expect(row.querySelector(".track-title")?.textContent?.trim()).toBe("Song Title");
    expect(row.querySelector(".track-subtitle")?.textContent).toContain("Artist Name");
    expect(row.querySelector(".track-subtitle")?.textContent).toContain("Album");
    row.querySelector<HTMLButtonElement>('[aria-label="Play Song Title"]')!.click();
    row.querySelector<HTMLButtonElement>('[aria-label="Add Song Title to favorites"]')!.click();
    row.querySelector<HTMLButtonElement>('[aria-label="Download Song Title"]')!.click();
    expect(onPlay).toHaveBeenCalledWith(tracks[0], tracks, 0); expect(onFavorite).toHaveBeenCalledWith(tracks[0]); expect(onDownload).toHaveBeenCalledWith(tracks[0]);
  });

  it("opens the exact track's album from its inline album name", () => {
    const onAlbum = vi.fn(), onRowSelect = vi.fn();
    const target = render({ onAlbum, onRowSelect });
    const albumLink = target.querySelector<HTMLButtonElement>('.album-link[aria-label="Open album Album"]')!;
    expect(albumLink.textContent).toBe("Album");
    albumLink.click();
    expect(onAlbum).toHaveBeenCalledWith(tracks[0]);
    expect(onRowSelect).not.toHaveBeenCalled();
  });

  it("announces the current row and exposes buffering and selection states", async () => {
    const onSelect = vi.fn();
    const target = render({ playingTrackId: "track-1", bufferingTrackId: "track-1", selectedTrackIds: ["track-1"], onSelect });
    const row = target.querySelector("li")!;
    expect(row.getAttribute("aria-current")).toBe("true");
    expect(row.classList.contains("buffering")).toBe(true);
    expect(row.classList.contains("selected")).toBe(true);
    expect(target.querySelector<HTMLInputElement>(".select-track")).toBeNull();
    target.querySelector<HTMLButtonElement>(".start-selection")!.click(); await tick();
    expect(target.querySelector<HTMLInputElement>('[aria-label="Select Song Title"]')?.checked).toBe(true);
    const selectionEvent = new MouseEvent("click", { bubbles: true, shiftKey: true });
    target.querySelector<HTMLInputElement>('[aria-label="Select Song Title"]')!.dispatchEvent(selectionEvent);
    expect(onSelect).toHaveBeenCalledWith(tracks[0], selectionEvent, 0);
  });

  it("supports controlled occurrence selection without collapsing duplicate track IDs", async () => {
    const duplicate = { ...tracks[0] };
    const duplicateRows = [tracks[0], duplicate];
    const onSelect = vi.fn();
    const target = render({
      tracks: duplicateRows,
      selectionMode: true,
      selectionKeys: ["track-1:0", "track-1:1"],
      selectedTrackIds: ["track-1:1"],
      selectedTrackId: "track-1",
      onSelect,
    });
    const checkboxes = [...target.querySelectorAll<HTMLInputElement>(".select-track")];
    expect(checkboxes.map((checkbox) => checkbox.checked)).toEqual([false, true]);
    const rows = [...target.querySelectorAll<HTMLLIElement>("li[data-track-index]")];
    expect(rows.map((row) => row.classList.contains("selected"))).toEqual([false, true]);
    const selectionEvent = new MouseEvent("click", { bubbles: true });
    checkboxes[1]!.dispatchEvent(selectionEvent);
    expect(onSelect).toHaveBeenCalledWith(duplicate, selectionEvent, 1);
    expect(target.querySelector(".playlist-selection-actions")).toBeNull();
  });

  it("distinguishes an unavailable source from paused and playing rows", () => {
    const onPlay=vi.fn();
    const target=render({playingTrackId:"track-1",playbackFailed:true,paused:true,onPlay});
    const status=target.querySelector(".playback-state")!;
    expect(status.getAttribute("aria-label")).toBe("Unavailable Song Title");
    expect(status.textContent).toBe("!");
    expect(status.querySelector(".playback-playing-icon")).toBeNull();
    expect(status.querySelector(".buffering-spinner")).toBeNull();
    expect(target.querySelector(".icon-action.play.buffering")).toBeNull();
    target.querySelector<HTMLButtonElement>(".track-title")!.click();
    expect(onPlay).toHaveBeenCalledWith(tracks[0],tracks,0);
  });

  it("shows distinct accessible current playback states without changing the play action", () => {
    const onPlay = vi.fn();
    const target = render({ playingTrackId: "track-1", onPlay });
    let status = target.querySelector<HTMLElement>(".playback-state")!;
    const playingIcon = status.querySelector<SVGSVGElement>(".playback-playing-icon");
    expect(playingIcon).not.toBeNull();
    expect(playingIcon?.querySelector(".playback-speaker-shape")).toBeNull();
    expect(playingIcon?.querySelectorAll("path")).toHaveLength(1);
    expect(status.textContent?.trim()).toBe("");
    expect(status.getAttribute("aria-label")).toBe("Playing Song Title");
    expect(status.querySelector(".buffering-spinner")).toBeNull();
    target.querySelector<HTMLButtonElement>('.track-title[aria-label="Play Song Title"]')!.click();
    expect(onPlay).toHaveBeenCalledWith(tracks[0], tracks, 0);

    resetRendered();
    const pausedTarget = render({ playingTrackId: "track-1", paused: true, onPlay });
    status = pausedTarget.querySelector<HTMLElement>(".playback-state")!;
    expect(status.textContent).toBe("Ⅱ");
    expect(status.getAttribute("aria-label")).toBe("Paused Song Title");
    expect(status.querySelector(".buffering-spinner")).toBeNull();

    resetRendered();
    const bufferingTarget = render({ playingTrackId: "track-1", bufferingTrackId: "track-1", onPlay });
    status = bufferingTarget.querySelector<HTMLElement>(".playback-state")!;
    expect(status.getAttribute("aria-label")).toBe("Buffering Song Title");
    expect(status.querySelector(".buffering-spinner")).not.toBeNull();
    expect(bufferingTarget.querySelector<HTMLButtonElement>(".icon-action.play")?.getAttribute("aria-busy")).toBe("true");
    expect(bufferingTarget.querySelectorAll(".buffering-spinner")).toHaveLength(1);
    expect(bufferingTarget.querySelector<HTMLButtonElement>(".icon-action.play")?.textContent?.trim()).toBe("▶");
    expect(bufferingTarget.querySelector<HTMLButtonElement>('.track-title[aria-label="Play Song Title"]')).toBeTruthy();
  });

  it("keeps a visible buffering status for a prebuffered non-current track in compact lists", () => {
    const other = { ...tracks[0]!, id: "other-track", title: "Other Track" };
    const target = render({ tracks: [tracks[0]!, other], playingTrack: other, bufferingTrack: tracks[0], onPlay: vi.fn() });
    const row = target.querySelector<HTMLLIElement>('[data-track-index="0"]')!;
    const status = row.querySelector<HTMLElement>(".playback-state")!;
    expect(row.classList.contains("current")).toBe(false);
    expect(row.classList.contains("buffering")).toBe(true);
    expect(status.getAttribute("aria-label")).toBe("Buffering Song Title");
    expect(status.querySelector(".buffering-spinner")).not.toBeNull();
  });

  it.each(["playlist", "default"] as const)("shows one contained buffering spinner for %s rows only while loading", async (presentation) => {
    const props = proxy({
      tracks,
      presentation,
      playingTrack: tracks[0],
      bufferingTrack: tracks[0],
      onPlay: vi.fn(),
    });
    const target = document.createElement("div"); document.body.append(target);
    const instance = mount(TrackList, { target, props: props as never }); cleanup = () => unmount(instance);
    await tick();

    const row = target.querySelector<HTMLElement>("li")!;
    const spinner = row.querySelector<SVGSVGElement>(".buffering-spinner")!;
    const loadingButton = row.querySelector<HTMLButtonElement>(presentation === "playlist" ? ".position-play" : ".icon-action.play")!;
    expect(row.querySelectorAll(".buffering-spinner")).toHaveLength(1);
    expect(loadingButton.getAttribute("aria-busy")).toBe("true");
    expect(loadingButton.getAttribute("aria-label")).toBe("Buffering Song Title");
    expect(trackListSource).toMatch(/\.icon-action\.play\.buffering::before\s*\{[^}]*animation:\s*track-row-buffering/);
    if (presentation === "playlist") expect(trackListSource).toMatch(/\.playlist li\.buffering \.position-play\s*\{\s*display:\s*grid\s*\}/);
    expect(spinner.getAttribute("viewBox")).toBe("0 0 20 20");
    expect(spinner.querySelectorAll("circle, path")).toHaveLength(2);

    props.bufferingTrack = null;
    for (let index = 0; index < 4; index++) await Promise.resolve();
    await tick();
    expect(row.querySelectorAll(".buffering-spinner")).toHaveLength(0);
    expect(loadingButton.getAttribute("aria-busy")).toBe("false");
    expect(loadingButton.getAttribute("aria-label")).toBe("Play Song Title");
  });

  it("uses full recording identity for playback state, including chapter source ranges", () => {
    const chapterOne: Track = { ...tracks[0]!, id: "chapter:mix:one", playbackRange: {
      parentTrackId: "mix", chapterId: "one", sourceUrl: "https://example.test/mix", startTime: 20, endTime: 40, parentTitle: "Mix",
    } };
    const sameChapterElsewhere: Track = { ...chapterOne, sourceUrl: "https://catalog.test/chapter" };
    const differentRange: Track = { ...chapterOne, playbackRange: { ...chapterOne.playbackRange!, startTime: 21 } };
    const target = render({ tracks: [chapterOne, sameChapterElsewhere, differentRange], playingTrack: { ...chapterOne }, bufferingTrack: { ...chapterOne }, playingTrackId: "legacy-other-id", bufferingTrackId: "legacy-other-id" });
    const rows = [...target.querySelectorAll<HTMLLIElement>("li")];
    expect(rows.map(row => row.classList.contains("current"))).toEqual([true, true, false]);
    expect(rows.map(row => row.classList.contains("buffering"))).toEqual([true, true, false]);
    expect(rows.map(row => row.getAttribute("aria-current"))).toEqual(["true", "true", null]);
  });

  it("adds selected repeated occurrences as the original track objects", async () => {
    const first = { ...tracks[0]!, provenance: { remoteEntryId: "entry-first" } };
    const second = { ...tracks[0]!, provenance: { remoteEntryId: "entry-second" } };
    const onAddSelectedToPlaylist = vi.fn();
    const target = render({ tracks: [first, second], onAddSelectedToPlaylist });
    expect(target.querySelector(".select-track")).toBeNull();
    target.querySelector<HTMLButtonElement>(".start-selection")!.click(); await tick();
    target.querySelectorAll<HTMLInputElement>('[aria-label="Select Song Title for playlist"]')[1]!.click();
    await tick();
    expect(target.textContent).toContain("1 tracks selected");
    target.querySelector<HTMLButtonElement>(".add-selected")!.click();
    expect(onAddSelectedToPlaylist).toHaveBeenCalledWith([second]);
    target.querySelectorAll<HTMLInputElement>('[aria-label="Select Song Title for playlist"]')[0]!.click();
    await tick();
    target.querySelector<HTMLButtonElement>(".add-selected")!.click();
    expect(onAddSelectedToPlaylist).toHaveBeenLastCalledWith([first, second]);
  });

  it("queues thirty selected occurrences once in visible order without dropping duplicate sources", async () => {
    const rows = Array.from({ length: 30 }, (_, index) => ({
      ...tracks[index % tracks.length]!,
      id: index < 2 ? "same-recording" : `recording-${index}`,
      provenance: { occurrence: `source-${index}` },
    }));
    const onQueueSelected = vi.fn<(items: Track[]) => void>();
    const target = render({ tracks: rows, onAddSelectedToPlaylist: vi.fn(), onQueueSelected });
    target.querySelector<HTMLButtonElement>(".start-selection")!.click(); await tick();
    const checkboxes = [...target.querySelectorAll<HTMLInputElement>(".select-track")];
    expect(checkboxes).toHaveLength(30);
    for (const checkbox of [...checkboxes].reverse()) checkbox.click();
    await tick();
    target.querySelector<HTMLButtonElement>(".queue-selected")!.click(); await tick();
    expect(onQueueSelected).toHaveBeenCalledTimes(1);
    const queued = onQueueSelected.mock.calls[0]![0]!;
    expect(queued).toHaveLength(30);
    queued.forEach((track, index) => expect(track).toBe(rows[index]));
    expect(queued[0]!.id).toBe(queued[1]!.id);
    expect(queued[0]!.provenance).toEqual({ occurrence: "source-0" });
    expect(queued[1]!.provenance).toEqual({ occurrence: "source-1" });
    expect(target.textContent).toContain("30 tracks selected");
  });

  it("disables Queue selected with no selected occurrences", async () => {
    const onQueueSelected = vi.fn<(items: Track[]) => void>();
    const target = render({ tracks: [tracks[0]!], onAddSelectedToPlaylist: vi.fn(), onQueueSelected });
    target.querySelector<HTMLButtonElement>(".start-selection")!.click(); await tick();
    expect(target.querySelector<HTMLButtonElement>(".queue-selected")?.disabled).toBe(true);
    expect(onQueueSelected).not.toHaveBeenCalled();
  });

  it("keeps selected occurrences and reports a failed batch queue action", async () => {
    const rows = [tracks[0]!, tracks[1]!];
    const onQueueSelected = vi.fn().mockRejectedValue(new Error("Queue limit reached"));
    const target = render({ tracks: rows, onAddSelectedToPlaylist: vi.fn(), onQueueSelected });
    target.querySelector<HTMLButtonElement>(".start-selection")!.click(); await tick();
    for (const checkbox of target.querySelectorAll<HTMLInputElement>(".select-track")) checkbox.click();
    await tick();
    target.querySelector<HTMLButtonElement>(".queue-selected")!.click(); await tick(); await tick();
    expect(target.querySelector('[role="alert"]')?.textContent).toContain("Queue limit reached");
    expect([...target.querySelectorAll<HTMLInputElement>(".select-track")].map(input => input.checked)).toEqual([true, true]);
  });

  it("ignores a pending queue rejection after canceling selection", async () => {
    let rejectQueue: (failure: Error) => void = () => {};
    const onQueueSelected = vi.fn(() => new Promise<void>((_resolve, reject) => { rejectQueue = reject; }));
    const target = render({ tracks: [tracks[0]!], onAddSelectedToPlaylist: vi.fn(), onQueueSelected });
    target.querySelector<HTMLButtonElement>(".start-selection")!.click(); await tick();
    target.querySelector<HTMLInputElement>(".select-track")!.click(); await tick();
    target.querySelector<HTMLButtonElement>(".queue-selected")!.click();
    expect(onQueueSelected).toHaveBeenCalledTimes(1);
    target.querySelector<HTMLButtonElement>(".cancel-selection")!.click(); await tick();
    rejectQueue(new Error("late queue error"));
    for (let index = 0; index < 8; index++) { await Promise.resolve(); await tick(); }
    target.querySelector<HTMLButtonElement>(".start-selection")!.click(); await tick();
    expect(target.querySelector('[role="alert"]')).toBeNull();
  });

  it("ignores a pending queue rejection after the list context changes", async () => {
    let rejectQueue: (failure: Error) => void = () => {};
    const onQueueSelected = vi.fn(() => new Promise<void>((_resolve, reject) => { rejectQueue = reject; }));
    const target = document.createElement("div"); document.body.append(target);
    const props = proxy({ tracks: [tracks[0]!], selectionContext: "first artist", onAddSelectedToPlaylist: vi.fn(), onQueueSelected });
    const instance = mount(TrackList, { target, props: props as never }); cleanup = () => unmount(instance);
    await tick(); target.querySelector<HTMLButtonElement>(".start-selection")!.click(); await tick();
    target.querySelector<HTMLInputElement>(".select-track")!.click(); await tick();
    target.querySelector<HTMLButtonElement>(".queue-selected")!.click();
    props.tracks = [tracks[1]!]; props.selectionContext = "second artist";
    for (let index = 0; index < 8; index++) { await Promise.resolve(); await tick(); }
    rejectQueue(new Error("late queue error"));
    for (let index = 0; index < 8; index++) { await Promise.resolve(); await tick(); }
    expect(target.querySelector('[role="alert"]')).toBeNull();
    expect(target.querySelector<HTMLButtonElement>(".start-selection")).not.toBeNull();
  });

  it("inverts local playlist selection across listed repeated occurrences", async () => {
    const first = { ...tracks[0]!, provenance: { remoteEntryId: "first" } };
    const second = { ...tracks[0]!, provenance: { remoteEntryId: "second" } };
    const target = render({ tracks: [first, second], onAddSelectedToPlaylist: vi.fn() });
    target.querySelector<HTMLButtonElement>(".start-selection")!.click(); await tick();
    target.querySelectorAll<HTMLInputElement>('[aria-label="Select Song Title for playlist"]')[0]!.click();
    await tick();
    target.querySelector<HTMLButtonElement>(".invert-selection")!.click(); await tick();
    expect([...target.querySelectorAll<HTMLInputElement>('[aria-label="Select Song Title for playlist"]')].map(input => input.checked)).toEqual([false, true]);
    expect([...target.querySelectorAll<HTMLLIElement>("li[data-track-index]")].map(row => row.classList.contains("selected"))).toEqual([false, true]);
    target.querySelector<HTMLButtonElement>(".cancel-selection")!.click(); await tick();
    expect([...target.querySelectorAll<HTMLLIElement>("li[data-track-index]")].map(row => row.classList.contains("selected"))).toEqual([false, false]);
  });

  it("passes the full listed occurrence scope to controlled selection inversion", async () => {
    const onInvertSelection = vi.fn();
    const target = render({
      tracks: [tracks[0], tracks[0]],
      selectionMode: true,
      selectionKeys: ["occurrence:first", "occurrence:second"],
      onSelect: vi.fn(), onInvertSelection,
    });
    await tick();
    expect(target.querySelectorAll<HTMLInputElement>(".select-track")).toHaveLength(2);
    expect(target.querySelector(".playlist-selection-actions")).not.toBeNull();
    target.querySelector<HTMLButtonElement>(".invert-selection")!.click();
    expect(onInvertSelection).toHaveBeenCalledWith(["occurrence:first", "occurrence:second"], [tracks[0], tracks[0]]);
  });

  it("renders ordered status rows between tracks while retaining source indices and selection order", async () => {
    const first = { ...tracks[0]!, id: "source-zero" };
    const second = { ...tracks[1]!, id: "source-one" };
    const onPlay = vi.fn(), onAddSelectedToPlaylist = vi.fn();
    const target = render({
      tracks: [first, second], orderedRows: [
        { kind: "track", key: "occurrence-one", trackIndex: 1 },
        { kind: "status", key: "unindexed-file", title: "Not indexed", detail: "Scan this file before playback." },
        { kind: "track", key: "occurrence-zero", trackIndex: 0 },
      ],
      selectionKeys: ["source-zero", "source-one"], selectionTracks: [first, second], selectionTrackKeys: ["source-zero", "source-one"],
      onPlay, onAddSelectedToPlaylist,
    });
    const rows = [...target.querySelectorAll<HTMLLIElement>(".shared-track-list > li:not(.virtual-spacer)")];
    expect(rows[0]?.querySelector(".track-title")?.textContent?.trim()).toBe("Other Song");
    expect(rows[1]?.textContent).toContain("Not indexed");
    expect(rows[1]?.textContent).toContain("Scan this file before playback.");
    expect(rows[2]?.querySelector(".track-title")?.textContent?.trim()).toBe("Song Title");
    expect(rows[1]?.querySelector("button,input,details")).toBeNull();
    rows[0]!.querySelector<HTMLButtonElement>(".track-title")!.click();
    expect(onPlay).toHaveBeenCalledWith(second, [first, second], 1);
    target.querySelector<HTMLButtonElement>(".start-selection")!.click(); await tick();
    const checkboxes = [...target.querySelectorAll<HTMLInputElement>(".select-track")];
    checkboxes[1]!.click(); checkboxes[0]!.click(); await tick();
    target.querySelector<HTMLButtonElement>(".add-selected")!.click();
    expect(onAddSelectedToPlaylist).toHaveBeenCalledWith([second, first]);
  });

  it("virtualizes an ordered row sequence using one row slot for each status entry", () => {
    const longTracks = Array.from({ length: 205 }, (_, index) => ({ ...tracks[index % tracks.length]!, id: `track-${index}` }));
    const orderedRows = longTracks.flatMap((_, trackIndex) => [
      ...(trackIndex === 1 ? [{ kind: "status" as const, key: "unindexed", title: "Unindexed file", detail: "Scan to make it playable." }] : []),
      { kind: "track" as const, key: `track-row-${trackIndex}`, trackIndex },
    ]);
    const target = render({ tracks: longTracks, orderedRows });
    expect(target.querySelector(".track-list-viewport")?.classList.contains("virtualized")).toBe(true);
    expect(target.querySelector(".track-status-row")?.textContent).toContain("Unindexed file");
    const spacers = target.querySelectorAll<HTMLElement>(".virtual-spacer");
    expect(spacers.length).toBe(1);
    expect(Number.parseInt(spacers[0]!.style.height, 10)).toBeGreaterThan(0);
    expect(target.querySelectorAll(".shared-track-list > li:not(.virtual-spacer)").length).toBeLessThan(orderedRows.length);
  });

  it("cancels selection mode, clears selected occurrences, and leaves row playback alone", async () => {
    const onPlay = vi.fn(); const onAddSelectedToPlaylist = vi.fn();
    const target = render({ tracks: [tracks[0]!], onPlay, onAddSelectedToPlaylist });
    target.querySelector<HTMLButtonElement>(".start-selection")!.click(); await tick();
    target.querySelector<HTMLInputElement>('[aria-label="Select Song Title for playlist"]')!.click();
    await tick();
    expect(target.querySelector(".add-selected")).not.toBeNull();
    expect(onPlay).not.toHaveBeenCalled();
    target.querySelector<HTMLButtonElement>(".cancel-selection")!.click();
    await tick();
    expect(target.querySelector(".select-track")).toBeNull();
    expect(target.querySelector(".add-selected")).toBeNull();
    expect(target.querySelector(".start-selection")).not.toBeNull();
    expect(onAddSelectedToPlaylist).not.toHaveBeenCalled();
    expect(onPlay).not.toHaveBeenCalled();
  });

  it("clears prior selection when a reused list changes to a new playlist context", async () => {
    const first = { ...tracks[0]!, id: "first-list-track" };
    const second = { ...tracks[1]!, id: "second-list-track" };
    const onAddSelectedToPlaylist = vi.fn();
    const target = document.createElement("div"); document.body.append(target);
    const props = proxy({ tracks: [first], selectionContext: "playlist:first", onAddSelectedToPlaylist });
    const instance = mount(TrackList, { target, props: props as never }); cleanup = () => unmount(instance);
    await tick();
    target.querySelector<HTMLButtonElement>(".start-selection")!.click(); await tick();
    const firstCheckbox = target.querySelector<HTMLInputElement>('[aria-label="Select Song Title for playlist"]')!;
    firstCheckbox.checked = true; firstCheckbox.dispatchEvent(new Event("change", { bubbles: true })); await tick();
    expect(target.textContent).toContain("1 tracks selected");
    props.tracks = [second]; props.selectionContext = "playlist:second";
    for (let index = 0; index < 8; index++) await Promise.resolve(); await tick();
    expect(target.querySelector<HTMLInputElement>('[aria-label="Select Other Song for playlist"]')).toBeNull();
    expect(target.querySelector(".add-selected")).toBeNull();
    expect(target.querySelector(".start-selection")).not.toBeNull();
  });

  it("continues to support legacy playing and buffering ID props", () => {
    const target = render({ playingTrackId: "track-1", bufferingTrackId: "track-2" });
    const rows = [...target.querySelectorAll<HTMLLIElement>("li")];
    expect(rows[0]?.classList.contains("current")).toBe(true);
    expect(rows[1]?.classList.contains("buffering")).toBe(true);
  });

  it("shows exact chapter range duration and omits unknown or invalid durations", () => {
    const ranged = { ...tracks[0]!, durationSeconds: 600, playbackRange: {
      parentTrackId: "mix", chapterId: "part", sourceUrl: "https://example.test/mix", startTime: 75, endTime: 120, parentTitle: "Mix",
    } };
    const target = render({ tracks: [ranged, tracks[1], { ...tracks[1]!, id: "invalid", durationSeconds: Number.NaN }] });
    expect([...target.querySelectorAll(".duration")].map(node => node.textContent)).toEqual(["0:45"]);
  });

  it("includes reduced-motion handling for the buffering indicator", () => {
    const target = render({ bufferingTrackId: "track-1", onPlay: vi.fn() });
    expect(target.querySelector(".buffering .playback-state .buffering-spinner")).toBeTruthy();
    expect(trackListSource).toMatch(/BufferingSpinner/);
    expect(bufferingSpinnerSource).toMatch(/prefers-reduced-motion\s*:\s*reduce/);
    expect(bufferingSpinnerSource).toMatch(/animation\s*:\s*none/);
    expect(trackListSource).toMatch(/prefers-reduced-motion\s*:\s*reduce/);
    expect(trackListSource).toMatch(/\.icon-action\.play\.buffering::before\s*\{[^}]*animation:\s*track-row-buffering/);
    expect(trackListSource).toMatch(/\.playback-state\.status-buffering-has-play\s*\{[^}]*position:\s*absolute/);
    expect(trackListSource).toMatch(/playback-state\.status-buffering-has-play\s*\{position:relative;width:22px/);
    expect(trackListSource).not.toMatch(/\.playback-state\.status-buffering-has-play\s*\{\s*display:\s*none/);
  });

  it("bounds a 10k row list and preserves full-list play indexes after scrolling", async () => {
    const largeTracks: Track[] = Array.from({ length: 10_000 }, (_, index) => ({
      ...tracks[0]!, id: `large-${index}`, providerId: `large-${index}`, title: `Track ${index}`,
    }));
    const onPlay = vi.fn();
    const target = render({ tracks: largeTracks, onPlay });
    const viewport = target.querySelector<HTMLElement>(".track-list-viewport")!;
    Object.defineProperty(viewport, "clientHeight", { configurable: true, value: 560 });
    let scrollTop = 88 * 8000;
    Object.defineProperty(viewport, "scrollTop", { configurable: true, get: () => scrollTop, set: value => { scrollTop = value; } });
    viewport.dispatchEvent(new Event("scroll", { bubbles: true }));
    await tick();
    expect(target.querySelectorAll("li:not(.virtual-spacer)").length).toBeLessThan(30);
    const row = [...target.querySelectorAll<HTMLLIElement>("li:not(.virtual-spacer)")]
      .find(item => item.textContent?.includes("Track 8000"))!;
    row.querySelector<HTMLButtonElement>('[aria-label="Play Track 8000"]')!.click();
    expect(onPlay).toHaveBeenCalledWith(largeTracks[8000], largeTracks, 8000);
  });

  it("measures the viewport on mount and resize, including the selection toolbar in scroll geometry", async () => {
    let observerCallback: ResizeObserverCallback | undefined;
    globalThis.ResizeObserver = class {
      constructor(callback: ResizeObserverCallback) { observerCallback = callback; }
      observe() {}
      unobserve() {}
      disconnect() {}
    } as unknown as typeof ResizeObserver;
    const largeTracks: Track[] = Array.from({ length: 10_000 }, (_, index) => ({
      ...tracks[0]!, id: `resize-${index}`, providerId: `resize-${index}`, title: `Resize ${index}`,
    }));
    const target = render({ tracks: largeTracks, selectionMode: true, onAddSelectedToPlaylist: vi.fn() });
    const viewport = target.querySelector<HTMLDivElement>(".track-list-viewport")!;
    const toolbar = target.querySelector<HTMLElement>(".playlist-selection-actions")!;
    let height = 300;
    Object.defineProperty(viewport, "clientHeight", { configurable: true, get: () => height });
    Object.defineProperty(toolbar, "offsetHeight", { configurable: true, value: 60 });
    await tick();
    observerCallback?.([{ target: viewport } as unknown as ResizeObserverEntry], {} as ResizeObserver);
    await tick();
    const initialRows = target.querySelectorAll("li[data-track-index]").length;
    expect(initialRows).toBeGreaterThan(12);
    expect(initialRows).toBeLessThan(30);

    height = 900;
    observerCallback?.([{ target: viewport } as unknown as ResizeObserverEntry], {} as ResizeObserver);
    await tick();
    expect(target.querySelectorAll("li[data-track-index]").length).toBeGreaterThan(initialRows);

    let scrollTop = 88 * 20 + 60;
    Object.defineProperty(viewport, "scrollTop", { configurable: true, get: () => scrollTop, set: value => { scrollTop = value; } });
    viewport.dispatchEvent(new Event("scroll"));
    await tick();
    const firstVisible = target.querySelector<HTMLLIElement>("li[data-track-index]")!;
    expect(Number(firstVisible.dataset.trackIndex)).toBeLessThanOrEqual(20);
    expect(Number(firstVisible.dataset.trackIndex)).toBeGreaterThanOrEqual(13);
  });

  it("keeps the list renderable when ResizeObserver is unavailable", async () => {
    const original = globalThis.ResizeObserver;
    Object.defineProperty(globalThis, "ResizeObserver", { configurable: true, value: undefined });
    try {
      const target = render({ tracks });
      await tick();
      expect(target.querySelectorAll("li[data-track-index]")).toHaveLength(tracks.length);
    } finally {
      Object.defineProperty(globalThis, "ResizeObserver", { configurable: true, value: original });
    }
  });

  it("restores the virtual window to the saved scroll position without browser scroll anchoring", async () => {
    const largeTracks: Track[] = Array.from({ length: 10_000 }, (_, index) => ({
      ...tracks[0]!, id: `restore-${index}`, providerId: `restore-${index}`, title: `Restore ${index}`,
    }));
    const target = render({ tracks: largeTracks, initialScroll: 88 * 8_000 });
    await tick();
    const viewport = target.querySelector<HTMLElement>(".track-list-viewport")!;
    expect(viewport.scrollTop).toBe(88 * 8_000);
    expect(Number(target.querySelector<HTMLLIElement>("li[data-track-index]")?.dataset.trackIndex)).toBeGreaterThan(7_990);
    expect(Number(target.querySelector<HTMLLIElement>("li[data-track-index]")?.dataset.trackIndex)).toBeLessThan(8_010);
    expect(trackListSource).toMatch(/overflow-anchor\s*:\s*none/);
  });

  it("offers keyboard and drag reorder only in Select mode, without arrow buttons", async () => {
    const rows: Track[] = [
      { ...tracks[0]!, id: "repeat", provenance: { playlistPosition: 1 } },
      { ...tracks[0]!, id: "repeat", provenance: { playlistPosition: 2 } },
      { ...tracks[1]!, provenance: { playlistPosition: 3 } },
    ];
    const onReorder = vi.fn().mockResolvedValue(undefined);
    const target = render({ tracks: rows, presentation: "playlist", onPlay: vi.fn(), onAddSelectedToPlaylist: vi.fn(), reorderable: true, onReorder, reorderCount: 3 });
    expect(target.querySelectorAll(".playlist-reorder-handle")).toHaveLength(0);
    expect(target.querySelector('[aria-label^="Move "]')).toBeNull();
    target.querySelector<HTMLButtonElement>(".start-selection")!.click(); await tick();
    const handles = [...target.querySelectorAll<HTMLButtonElement>(".playlist-reorder-handle")];
    expect(handles).toHaveLength(3);
    expect(target.querySelector('[aria-label^="Move "]')).toBeNull();

    const arrow = (handle: HTMLButtonElement, key: "ArrowUp" | "ArrowDown") => {
      const event = new KeyboardEvent("keydown", { key, bubbles: true, cancelable: true });
      handle.dispatchEvent(event);
      return event;
    };
    expect(arrow(handles[0]!, "ArrowUp").defaultPrevented).toBe(true);
    await tick();
    expect(onReorder).not.toHaveBeenCalled();

    expect(handles[0]!.getAttribute("aria-keyshortcuts")).toBe("ArrowUp ArrowDown");
    expect(arrow(handles[0]!, "ArrowDown").defaultPrevented).toBe(true);
    await tick();
    expect(onReorder).toHaveBeenCalledWith(0, 1);
    await Promise.resolve(); await tick();
    expect(target.querySelector('[aria-live="polite"]')?.textContent).toContain("Moved Song Title to position 2");

    onReorder.mockRejectedValueOnce(new Error("offline"));
    arrow(handles[0]!, "ArrowDown"); await Promise.resolve(); await tick();
    expect(target.querySelector('[aria-live="polite"]')?.textContent).toContain("Could not move Song Title");

    const last = handles[2]!;
    arrow(last, "ArrowDown"); await tick();
    expect(onReorder).toHaveBeenCalledTimes(2);

    target.querySelector<HTMLButtonElement>(".cancel-selection")!.click(); await tick();
    expect(target.querySelectorAll(".playlist-reorder-handle")).toHaveLength(0);
    expect(target.querySelector('[aria-label^="Move "]')).toBeNull();
  });

  it("disables reorder controls when listed occurrences are filtered or sorted", () => {
    const filteredRows: Track[] = [
      { ...tracks[0]!, provenance: { playlistPosition: 1 } },
      { ...tracks[1]!, provenance: { playlistPosition: 3 } },
    ];
    const filteredTarget = render({ tracks: filteredRows, reorderable: true, onReorder: vi.fn(), reorderCount: 3 });
    expect(filteredTarget.querySelector(".playlist-reorder-handle")).toBeNull();
    resetRendered();
    const rows: Track[] = [
      { ...tracks[0]!, provenance: { playlistPosition: 2 } },
      { ...tracks[1]!, provenance: { playlistPosition: 1 } },
    ];
    const target = render({ tracks: rows, reorderable: true, onReorder: vi.fn(), reorderCount: 3 });
    expect(target.querySelector(".playlist-reorder-handle")).toBeNull();
    expect(target.querySelector('[aria-label^="Move "]')).toBeNull();
  });
});

it("uses compact numbered playlist rows without putting listening dates on a third text line", async () => {
    const onPlay = vi.fn();
    const target = render({presentation: "playlist", onPlay, timestamp: () => ({label: "Oct 18, 2026", full: "Listening time"})});
    expect(target.querySelector(".track-list-viewport")?.classList.contains("playlist")).toBe(true);
    const row = target.querySelector(".shared-track-list li")!;
    expect(row.querySelector(".track-position")?.textContent).toBe("1");
    expect(row.querySelector(".track-copy .track-time")).toBeNull();
    expect(row.querySelector(":scope > .track-time")?.textContent).toBe("Oct 18, 2026");
    expect(row.querySelector(".track-subtitle")?.textContent).toContain("Artist Name");
    expect(row.querySelector(".track-subtitle")?.textContent).toContain("Album");
    row.querySelector<HTMLButtonElement>(".position-play")!.click();
    expect(onPlay).toHaveBeenCalledWith(tracks[0], tracks, 0);
    await tick();
  });

  it.each([
    ["available", "Downloaded"],
    ["missing", "Missing file"],
    ["none", null],
    ["", null],
    [undefined, null],
  ] as const)("shows only known local file availability %s", (availability, expected) => {
    const target = render({ tracks: [{ ...tracks[0]!, localAvailability: availability }] });
    const badge = target.querySelector<HTMLElement>(".local-asset-status");
    if (expected) {
      expect(badge?.textContent).toBe(expected);
      expect(badge?.classList.contains("local-asset-missing")).toBe(expected === "Missing file");
    } else {
      expect(badge).toBeNull();
    }
  });

  it("keeps compact playlist virtualization aligned with its 60 pixel rows", async () => {
    const longTracks = Array.from({length: 300}, (_, index) => ({...tracks[0]!, id: `compact-${index}`, title: `Compact ${index}`}));
    const onPlay = vi.fn();
    const target = render({presentation: "playlist", tracks: longTracks, onPlay});
    const viewport = target.querySelector<HTMLDivElement>(".track-list-viewport")!;
    viewport.scrollTop = 6000; viewport.dispatchEvent(new Event("scroll")); await tick();
    const first = target.querySelector(".shared-track-list li:not(.virtual-spacer)")!;
    expect(first.querySelector(".track-position")?.textContent).toBe("95");
    expect(target.querySelector<HTMLElement>(".virtual-spacer")?.style.height).toBe("5640px");
    first.querySelector<HTMLButtonElement>(".position-play")!.click();
    expect(onPlay).toHaveBeenCalledWith(longTracks[94], longTracks, 94);
  });
