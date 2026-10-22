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
import type { Track } from "./lib/types";
import { playlistDropIndex } from "./lib/playlist-reorder";

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
    const target = render({ tracks: rows, reorderable: true, onReorder });
    const handle = target.querySelector<HTMLButtonElement>('[aria-label="Reorder Song Title at position 2"]')!;
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
      onSelect,
    });
    const checkboxes = [...target.querySelectorAll<HTMLInputElement>(".select-track")];
    expect(checkboxes.map((checkbox) => checkbox.checked)).toEqual([false, true]);
    const selectionEvent = new MouseEvent("click", { bubbles: true });
    checkboxes[1]!.dispatchEvent(selectionEvent);
    expect(onSelect).toHaveBeenCalledWith(duplicate, selectionEvent, 1);
    expect(target.querySelector(".playlist-selection-actions")).toBeNull();
  });

  it("shows distinct accessible current playback states without changing the play action", () => {
    const onPlay = vi.fn();
    const target = render({ playingTrackId: "track-1", onPlay });
    let status = target.querySelector<HTMLElement>(".playback-state")!;
    expect(status.textContent).toBe("♫");
    expect(status.getAttribute("aria-label")).toBe("Playing Song Title");
    target.querySelector<HTMLButtonElement>('.track-title[aria-label="Play Song Title"]')!.click();
    expect(onPlay).toHaveBeenCalledWith(tracks[0], tracks, 0);

    resetRendered();
    const pausedTarget = render({ playingTrackId: "track-1", paused: true, onPlay });
    status = pausedTarget.querySelector<HTMLElement>(".playback-state")!;
    expect(status.textContent).toBe("Ⅱ");
    expect(status.getAttribute("aria-label")).toBe("Paused Song Title");

    resetRendered();
    const bufferingTarget = render({ playingTrackId: "track-1", bufferingTrackId: "track-1", onPlay });
    status = bufferingTarget.querySelector<HTMLElement>(".playback-state")!;
    expect(status.textContent).toBe("…");
    expect(status.getAttribute("aria-label")).toBe("Buffering Song Title");
    expect(bufferingTarget.querySelector<HTMLButtonElement>('.track-title[aria-label="Play Song Title"]')).toBeTruthy();
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

  it("inverts local playlist selection across listed repeated occurrences", async () => {
    const first = { ...tracks[0]!, provenance: { remoteEntryId: "first" } };
    const second = { ...tracks[0]!, provenance: { remoteEntryId: "second" } };
    const target = render({ tracks: [first, second], onAddSelectedToPlaylist: vi.fn() });
    target.querySelector<HTMLButtonElement>(".start-selection")!.click(); await tick();
    target.querySelectorAll<HTMLInputElement>('[aria-label="Select Song Title for playlist"]')[0]!.click();
    await tick();
    target.querySelector<HTMLButtonElement>(".invert-selection")!.click(); await tick();
    expect([...target.querySelectorAll<HTMLInputElement>('[aria-label="Select Song Title for playlist"]')].map(input => input.checked)).toEqual([false, true]);
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
    expect(target.querySelector(".buffering .icon-action.play")).toBeTruthy();
    expect(trackListSource).toMatch(/prefers-reduced-motion\s*:\s*reduce/);
    expect(trackListSource).toMatch(/buffering[^\n]+animation\s*:\s*none/);
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
