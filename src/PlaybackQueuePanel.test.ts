// @vitest-environment jsdom
// @ts-expect-error Exercise Svelte's browser runtime in jsdom.
import { mount, tick, unmount } from "../node_modules/svelte/src/index-client.js";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
// @ts-expect-error Match the browser runtime used by the direct import above.
vi.mock("svelte", async () => await import("../node_modules/svelte/src/index-client.js"));
import PlaybackQueuePanel from "./PlaybackQueuePanel.svelte";
import type { Track } from "./lib/types";

const track: Track = { id: "queue-1", provider: "library", providerId: "queue-1", sourceUrl: "https://example.test/track", artist: "Queue Artist", title: "Queued Track", favorite: false, provenance: {} };
const radioTrack: Track = { id: "radio-1", provider: "radio", providerId: "radio-1", sourceUrl: "https://example.test/live", artist: "Radio Artist", title: "Known On Air Song", favorite: false, provenance: {} };
const heardTrack: Track = { ...radioTrack, id: "heard-1", title: "Previously Heard Song" };
const originalResizeObserver = globalThis.ResizeObserver;
const originalShowModal = Object.getOwnPropertyDescriptor(HTMLDialogElement.prototype, "showModal");
beforeEach(() => {
  globalThis.ResizeObserver = class { observe() {} unobserve() {} disconnect() {} } as unknown as typeof ResizeObserver;
  Object.defineProperty(HTMLDialogElement.prototype, "showModal", { configurable: true, value: function(this: HTMLDialogElement) { this.setAttribute("open", ""); } });
});
let cleanup: (() => void) | undefined;
afterEach(() => {
  cleanup?.(); cleanup = undefined; document.body.innerHTML = "";
  globalThis.ResizeObserver = originalResizeObserver;
  if (originalShowModal) Object.defineProperty(HTMLDialogElement.prototype, "showModal", originalShowModal);
  else Reflect.deleteProperty(HTMLDialogElement.prototype, "showModal");
});

function render(props: Record<string, unknown> = {}) {
  const target = document.createElement("div"); document.body.append(target);
  const instance = mount(PlaybackQueuePanel, { target, props: {
    tracks: [track], currentIndex: 0, position: 0, repeatMode: "off", shuffleEnabled: false,
    onClose: vi.fn(), onPlay: vi.fn(), onMove: vi.fn(), onRemove: vi.fn(), onSeek: vi.fn(),
    onToggleShuffle: vi.fn(), onCycleRepeat: vi.fn(), ...props,
  } as never });
  cleanup = () => unmount(instance);
  return target;
}

describe("PlaybackQueuePanel", () => {
  it("requests saved row data only when a queue row enters the viewport", async () => {
    let notify!: IntersectionObserverCallback;
    const nodes: Element[] = [];
    const onVisibleTrack = vi.fn();
    vi.stubGlobal("IntersectionObserver", class {
      constructor(callback: IntersectionObserverCallback) { notify = callback; }
      observe(node: Element) { if (!nodes.includes(node)) nodes.push(node); }
      unobserve() {} disconnect() {}
    });
    try {
      const second = {...track,id:"queue-2",title:"Second"};
      render({tracks:[track,second],onVisibleTrack});
      await tick();
      expect(onVisibleTrack).not.toHaveBeenCalled();
      notify([{target:nodes[1]!,isIntersecting:true} as IntersectionObserverEntry], {} as IntersectionObserver);
      await tick();
      expect(onVisibleTrack).toHaveBeenCalledExactlyOnceWith(second);
    } finally { cleanup?.(); cleanup=undefined; vi.unstubAllGlobals(); }
  });

  it("filters downloaded rows while retaining original queue occurrence indexes", async () => {
    const onPlay=vi.fn();
    const downloaded={...track,id:"downloaded",title:"Downloaded recording",localAvailability:"available"};
    const missing={...track,id:"missing",title:"Missing recording",localAvailability:"missing",localPath:"/missing.flac"};
    const rows=[track,downloaded,missing];const target=render({tracks:rows,currentIndex:0,onPlay});await tick();
    const select=target.querySelector<HTMLSelectElement>('[aria-label="Downloaded status"]')!;
    select.value="downloaded";select.dispatchEvent(new Event("change",{bubbles:true}));await tick();
    expect(target.querySelectorAll('.queue-list article')).toHaveLength(1);
    target.querySelector<HTMLButtonElement>('[aria-label="Play Downloaded recording"]')!.click();expect(onPlay).toHaveBeenCalledWith(1);
    select.value="not-downloaded";select.dispatchEvent(new Event("change",{bubbles:true}));await tick();
    expect(target.querySelector('[aria-label="Play Downloaded recording"]')).toBeNull();
    expect(target.querySelector('[aria-label="Play Missing recording"]')).not.toBeNull();
    expect(rows).toHaveLength(3);
  });

  it("returns focus to the queue action trigger when Escape closes its overflow menu", async () => {
    const target = render();
    const details = target.querySelector<HTMLDetailsElement>("details.queue-more")!;
    const summary = details.querySelector<HTMLElement>("summary")!;
    summary.click(); await tick();
    await tick();
    const action = details.querySelector<HTMLButtonElement>(".queue-more-menu button")!;
    action.focus();
    const escape = new KeyboardEvent("keydown", { key: "Escape", bubbles: true, cancelable: true });
    action.dispatchEvent(escape);
    await tick();
    expect(escape.defaultPrevented).toBe(true);
    expect(details.open).toBe(false);
    expect(document.activeElement).toBe(summary);
  });

  it("closes the overflow menu on resize without moving focus to its trigger", async () => {
    const target = render();
    const details = target.querySelector<HTMLDetailsElement>("details.queue-more")!;
    details.querySelector<HTMLElement>("summary")!.click(); await tick();
    await tick();
    expect(details.open).toBe(true);
    const sentinel = document.createElement("button"); sentinel.textContent = "Resize focus sentinel"; document.body.append(sentinel); sentinel.focus();
    window.dispatchEvent(new Event("resize"));
    await tick();
    expect(details.open).toBe(false);
    expect(document.activeElement).toBe(sentinel);
  });

  it("uses the shared action frame and keyboard navigation without enabling current-track removal", async () => {
    const target = render({ tracks: [track, { ...track, id: "next", title: "Next" }, { ...track, id: "last", title: "Last" }], currentIndex: 1 });
    const summary=target.querySelectorAll<HTMLElement>(".queue-list article summary")[1]!;
    summary.click(); await tick();
    const menu=target.querySelector<HTMLDialogElement>("dialog.action-menu.queue-more-menu")!;
    expect(menu.getAttribute("role")).toBe("menu");
    expect(menu.getAttribute("aria-label")).toBe("Actions for Next");
    expect(summary.getAttribute("aria-expanded")).toBe("true");
    const actions=[...menu.querySelectorAll<HTMLButtonElement>(":scope > button")];
    const remove=actions.find(button=>button.textContent?.trim()==="Remove from queue")!;
    expect(remove.disabled).toBe(true);
    for(const action of actions) Object.defineProperty(action,"getClientRects",{value:()=>[{width:100,height:32}]});
    menu.dispatchEvent(new KeyboardEvent("keydown",{key:"Home",bubbles:true,cancelable:true}));
    expect(document.activeElement?.textContent?.trim()).toBe("Move up");
    menu.dispatchEvent(new KeyboardEvent("keydown",{key:"End",bubbles:true,cancelable:true}));
    expect(document.activeElement?.textContent?.trim()).toBe("Clear below");
    menu.dispatchEvent(new KeyboardEvent("keydown",{key:"Escape",bubbles:true,cancelable:true}));await tick();
    expect(target.querySelector("dialog.action-menu")).toBeNull();
    expect(summary.getAttribute("aria-expanded")).toBe("false");
    expect(document.activeElement).toBe(summary);
  });

  it("labels the panel as the queue and provides info for queued tracks", async () => {
    const onInfoTrack = vi.fn();
    const target = render({ onInfoTrack });
    expect(target.querySelector(".queue-header h2")?.textContent).toContain("Queue");
    expect(target.querySelector(".queue-header h2")?.textContent).not.toContain("Up next");
    const more = target.querySelector<HTMLElement>('[aria-label="More actions for Queued Track"]')!;
    more.click(); await tick();
    const infoAction = [...target.querySelectorAll<HTMLButtonElement>('.queue-more-menu button')].find(button => button.textContent?.trim() === "Track information")!;
    expect(infoAction.querySelector('svg[aria-hidden="true"]')).not.toBeNull();
    infoAction.click();
    expect(onInfoTrack).toHaveBeenCalledWith(track);
  });

  it("adds the chosen queue occurrence to a playlist without losing its chapter range", async () => {
    const firstRange = { parentTrackId: track.id, chapterId: "chapter-1", sourceUrl: track.sourceUrl, startTime: 0, endTime: 60, parentTitle: track.title };
    const secondRange = { ...firstRange, chapterId: "chapter-2", startTime: 60, endTime: 120 };
    const firstOccurrence = { ...track, playbackRange: firstRange };
    const secondOccurrence = { ...track, playbackRange: secondRange };
    const onAddToPlaylist = vi.fn();
    const onRemove = vi.fn();
    const onPlay = vi.fn();
    const target = render({ tracks: [firstOccurrence, secondOccurrence], currentIndex: 0, onAddToPlaylist, onRemove, onPlay });

    const menus = target.querySelectorAll<HTMLDetailsElement>('.queue-list article details.queue-more');
    menus[1]!.querySelector<HTMLDetailsElement>("summary")!.click(); await tick();
    const addAction = [...menus[1]!.querySelectorAll<HTMLButtonElement>(".queue-more-menu button")].find(button => button.textContent?.trim() === "Add to playlist")!;
    expect(addAction.classList.contains("primary-action")).toBe(true);
    expect(addAction.querySelector('svg[aria-hidden="true"]')).not.toBeNull();
    addAction.click();

    expect(onAddToPlaylist).toHaveBeenCalledOnce();
    expect(onAddToPlaylist).toHaveBeenCalledWith(secondOccurrence, 1);
    expect((onAddToPlaylist.mock.calls[0]![0] as Track).playbackRange).toEqual(secondRange);
    expect(target.querySelectorAll(".queue-list article")).toHaveLength(2);
    expect(target.querySelectorAll(".queue-list article.current")).toHaveLength(1);
    expect(onRemove).not.toHaveBeenCalled();
    expect(onPlay).not.toHaveBeenCalled();
  });

  it("adds the selected duplicate parent occurrence's exact chapter to a playlist", async () => {
    const parent = { ...track, id: "set-parent", title: "Long Set" };
    const chapter = { id: "song-1", title: "Artist — Song", startTime: 60, endTime: 180, confidence: "review" } as const;
    const onAddChapterToPlaylist = vi.fn();
    const target = render({ tracks: [parent, parent], currentIndex: 1, chaptersByTrack: { [parent.id]: [chapter] }, onAddChapterToPlaylist });
    target.querySelector<HTMLElement>('[aria-label="More actions for Long Set"]')!.click(); await tick();
    [...target.querySelectorAll<HTMLButtonElement>(".queue-more-menu button")].find(button => button.textContent?.trim() === "Show chapters")!.click();
    await tick();
    const childActions = target.querySelectorAll<HTMLDetailsElement>('.chapter-children details.queue-more');
    expect(childActions).toHaveLength(2);
    childActions[1]!.querySelector("summary")!.click(); await tick();
    const add = [...childActions[1]!.querySelectorAll<HTMLButtonElement>(".queue-more-menu button")].find(button => button.textContent?.trim() === "Add chapter to playlist")!;
    expect(add.classList.contains("primary-action")).toBe(true);
    expect(add.querySelector('svg[aria-hidden="true"]')).not.toBeNull();
    add.click();
    expect(onAddChapterToPlaylist).toHaveBeenCalledOnce();
    expect(onAddChapterToPlaylist).toHaveBeenCalledWith(parent, chapter, 1);
  });

  it("shows cached detections beneath their queue track with seek, info, and playlist actions", async () => {
    const observation={title:"Recognized Song",artist:"Recognized Artist",detectedAtSeconds:125,capturedStartSeconds:120,capturedEndSeconds:132,observedAt:"2026-10-28T12:00:00Z",identifiers:{isrc:"USABC2600001"}};
    const onSeekDetection=vi.fn(),onInfoDetection=vi.fn(),onAddDetectionToPlaylist=vi.fn(),onAddDetectionsToPlaylist=vi.fn();
    const target=render({detectionsByTrack:{[track.id]:[observation]},onSeekDetection,onInfoDetection,onAddDetectionToPlaylist,onAddDetectionsToPlaylist});
    expect(target.querySelector('[aria-label="Detected tracks for Queued Track"]')?.textContent).toContain("Recognized Song");
    target.querySelector<HTMLButtonElement>('[aria-label^="Seek to"]')!.click();
    target.querySelector<HTMLButtonElement>(".detection-track-info")!.click();
    target.querySelector<HTMLButtonElement>(".detection-add")!.click();
    target.querySelector<HTMLButtonElement>(".detection-child-heading button")!.click();
    expect(onSeekDetection).toHaveBeenCalledWith(track,observation,0);
    expect(onInfoDetection).toHaveBeenCalledWith(expect.objectContaining({id:expect.stringMatching(/^detected:/),title:"Recognized Song",isrc:"USABC2600001"}));
    expect(onAddDetectionToPlaylist).toHaveBeenCalledWith(expect.objectContaining({title:"Recognized Song"}));
    expect(onAddDetectionsToPlaylist).toHaveBeenCalledWith([expect.objectContaining({title:"Recognized Song"})]);
  });

  it("shows the current radio song with an identify action and retains previous listens with normal actions", async () => {
    const onIdentifyRadio = vi.fn(), onPlayRadioTrack = vi.fn(), onContextRadioTrack = vi.fn();
    const target = render({
      radioNowPlaying: radioTrack, radioStationName: "Late Night FM", radioRecognizing: false, onIdentifyRadio,
      radioHistory: [{ id: "listen-occurrence-1", track: heardTrack, playedAt: "2026-10-14T12:30:00Z", stationName: "Late Night FM" }],
      onPlayRadioTrack, onContextRadioTrack,
    });
    expect(target.querySelector('[aria-label="Radio currently playing"]')?.textContent).toContain("Known On Air Song");
    expect(target.querySelector('[aria-label="Radio currently playing"]')?.textContent).toContain("Late Night FM");
    target.querySelector<HTMLButtonElement>(".identify")!.click();
    expect(onIdentifyRadio).toHaveBeenCalledOnce();

    target.querySelector<HTMLButtonElement>(".history-toggle")!.click();
    await tick();
    expect(target.querySelector(".history-list")?.textContent).toContain("Previously Heard Song");
    expect(target.querySelector(".history-list")?.textContent).toContain("Late Night FM");
    target.querySelector<HTMLButtonElement>('[aria-label="Play Previously Heard Song"]')!.click();
    target.querySelector<HTMLButtonElement>('[aria-label="Actions for Previously Heard Song"]')!.click();
    target.querySelector<HTMLButtonElement>('[aria-label="Actions for Known On Air Song"]')!.click();
    expect(onPlayRadioTrack).toHaveBeenCalledWith(heardTrack);
    expect(onContextRadioTrack).toHaveBeenNthCalledWith(1, heardTrack);
    expect(onContextRadioTrack).toHaveBeenNthCalledWith(2, radioTrack);
  });

  it("disables identify while recognition is running", async () => {
    const target = render({ radioNowPlaying: radioTrack, radioRecognizing: true, onIdentifyRadio: vi.fn() });
    expect(target.querySelector<HTMLButtonElement>(".identify")?.disabled).toBe(true);
  });


  it("identifies active non-radio audio from the expanded queue and disables while busy", async () => {
    const onIdentifyCurrent = vi.fn();
    const target = render({ onIdentifyCurrent, identifyingCurrentAudio: false });
    const identify = target.querySelector<HTMLButtonElement>('[aria-label="Identify currently playing audio"]');
    expect(identify?.textContent).toContain("Identify audio");
    identify!.click();
    expect(onIdentifyCurrent).toHaveBeenCalledOnce();

    cleanup?.(); cleanup = undefined; document.body.innerHTML = "";
    const busyTarget = render({ onIdentifyCurrent, identifyingCurrentAudio: true });
    const busyIdentify = busyTarget.querySelector<HTMLButtonElement>(".identify");
    expect(busyIdentify?.disabled).toBe(true);
    expect(busyIdentify?.textContent).toContain("Identifying");
  });

  it("does not show the generic identify action unless current audio is eligible", async () => {
    const target = render();
    expect(target.querySelector('[aria-label="Identify currently playing audio"]')).toBeNull();
  });

  it("keeps the queue row usable when artist and duration metadata are absent", async () => {
    const target = render({ tracks: [{ ...track, artist: "", durationSeconds: undefined }] });
    expect(target.querySelector(".track-copy small")?.textContent).toBe("Unknown artist");
    expect(target.querySelector(".track-duration")).toBeNull();
    expect(target.querySelector('[aria-label="Playing Queued Track"]')).toBeTruthy();
    expect(target.querySelectorAll("article > .queue-more")).toHaveLength(1);
    expect(target.querySelector(".row-controls")).toBeNull();
    expect(target.querySelector(".queue-controls")).toBeNull();
  });

  it("moves and removes queued tracks from the overflow menu", async () => {
    const onMove = vi.fn(), onRemove = vi.fn();
    const target = render({ tracks: [track, { ...track, id: "queue-2", title: "Next Track" }], currentIndex: 0, onMove, onRemove });
    const more = target.querySelector<HTMLElement>('[aria-label="More actions for Next Track"]')!;
    more.click(); await tick();
    const menu = more.closest("details")!.querySelector(".queue-more-menu")!;
    const buttons = [...menu.querySelectorAll<HTMLButtonElement>("button")];
    buttons.find(button => button.textContent === "Move up")!.click();
    expect(onMove).toHaveBeenCalledWith(1, 0);
    more.click(); await tick();
    more.closest("details")!.querySelector<HTMLButtonElement>('.queue-more-menu button[title="Remove from queue"]')!.click();
    expect(onRemove).toHaveBeenCalledWith(1);
  });

  it("supports modifier selection, inverse selection, and removing exact selected occurrences", async () => {
    const onRemoveMany = vi.fn();
    const rows = [track, { ...track, title: "Duplicate occurrence" }, { ...track, title: "Last occurrence" }];
    const target = render({ tracks: rows, onRemoveMany });
    await tick();
    const tracks = target.querySelectorAll<HTMLButtonElement>(".queue-list article .queue-drag-select");
    tracks[1]!.dispatchEvent(new MouseEvent("click", { bubbles: true, ctrlKey: true }));
    await tick();
    const selects = target.querySelectorAll<HTMLButtonElement>(".queue-list article .queue-drag-select");
    expect(selects[1]!.getAttribute("aria-pressed")).toBe("true");
    target.querySelector<HTMLButtonElement>('.queue-selection button')!.click();
    await tick();
    expect([...target.querySelectorAll<HTMLElement>(".queue-list article")].map(row => row.classList.contains("queue-selected"))).toEqual([true, false, true]);
    target.querySelector<HTMLButtonElement>('.queue-selection button:last-child')!.click();
    expect(onRemoveMany).toHaveBeenCalledWith([2]);
  });

  it("clears all occurrences and clears only rows below the chosen row", async () => {
    const onRemoveMany = vi.fn();
    const rows = [track, { ...track, id: "queue-2", title: "Next" }, { ...track, id: "queue-3", title: "Last" }];
    const target = render({ tracks: rows, onRemoveMany });
    target.querySelector<HTMLButtonElement>(".clear-queue")!.click();
    expect(onRemoveMany).toHaveBeenLastCalledWith([2, 1]);

    const more = target.querySelector<HTMLElement>('[aria-label="More actions for Queued Track"]')!;
    more.click(); await tick();
    const clearBelow = [...target.querySelectorAll<HTMLButtonElement>(".queue-more-menu button")].find(button => button.textContent?.trim() === "Clear below")!;
    clearBelow.click();
    expect(onRemoveMany).toHaveBeenLastCalledWith([2, 1]);
  });

  it("dispatches an explicit upcoming sort using the selected shared sort field and direction", async () => {
    const onSortUpcoming = vi.fn();
    const target = render({
      tracks: [track, { ...track, id: "queue-2", title: "Next Track" }, { ...track, id: "queue-3", title: "Another Next Track" }],
      currentIndex: 0,
      onSortUpcoming,
    });
    const field = target.querySelector<HTMLSelectElement>('[aria-label="Sort upcoming by"]')!;
    const direction = target.querySelector<HTMLSelectElement>('[aria-label="Sort direction"]')!;
    field.value = "artist";
    field.dispatchEvent(new Event("change", { bubbles: true }));
    direction.value = "desc";
    direction.dispatchEvent(new Event("change", { bubbles: true }));
    await tick();
    target.querySelector<HTMLButtonElement>(".queue-sort-upcoming button")!.click();
    expect(onSortUpcoming).toHaveBeenCalledWith("artist", "desc");
  });

  it("hides upcoming sorting when only one track remains after the current occurrence", async () => {
    const target = render({
      tracks: [track, { ...track, id: "queue-2", title: "Final Upcoming Track" }],
      currentIndex: 0,
      onSortUpcoming: vi.fn(),
    });
    expect(target.querySelector(".queue-sort-upcoming")).toBeNull();
  });

  it("marks only the current queue occurrence and its canonical chapter as buffering", async () => {
    const chapter = { id: "chapter-1", title: "First Chapter", startTime: 0, endTime: 120, confidence: "high" };
    const duplicate = { ...track };
    const range = { parentTrackId: duplicate.id, chapterId: chapter.id, sourceUrl: "https://source.test/audio", startTime: 0, endTime: 120, parentTitle: duplicate.title };
    const activePlaybackTrack = { ...duplicate, id: `chapter:${duplicate.id}:${chapter.id}`, playbackRange: range };
    const target = render({
      tracks: [track, duplicate], currentIndex: 1, chaptersByTrack: { [track.id]: [chapter], [duplicate.id]: [chapter] },
      chapterRangesByTrack: { [track.id]: { [chapter.id]: { ...range, parentTrackId: track.id } }, [duplicate.id]: { [chapter.id]: range } },
      chapters: [chapter], position: 20, activePlaybackTrack, buffering: true,
    });
    const firstMenu = target.querySelector<HTMLElement>(".queue-list article summary")!;
    firstMenu.click(); await tick();
    [...firstMenu.closest("article")!.querySelectorAll<HTMLButtonElement>(".queue-more-menu button")].find(button => button.textContent === "Show chapters")?.click();
    await tick();
    const queueRows = [...target.querySelectorAll<HTMLElement>(".queue-list article")];
    expect(queueRows.map(row => row.classList.contains("current"))).toEqual([false, true]);
    expect(queueRows.map(row => row.getAttribute("aria-busy"))).toEqual([null, "true"]);
    expect(queueRows.map(row => row.querySelector(".track-index")?.textContent)).toEqual(["01", "…"]);
    expect(queueRows[1].querySelector(".track-select")?.getAttribute("aria-label")).toBe("Buffering Queued Track");
    const childRows = [...target.querySelectorAll<HTMLElement>(".chapter-child")];
    expect(childRows.map(row => row.classList.contains("current"))).toEqual([false, true]);
    expect(childRows.map(row => row.getAttribute("aria-busy"))).toEqual([null, "true"]);
    const activeChapterButton = target.querySelector<HTMLButtonElement>(".chapter-list .chapter-play")!;
    expect(activeChapterButton.getAttribute("aria-current")).toBe("time");
    expect(activeChapterButton.textContent).toContain("…");
    expect(activeChapterButton.getAttribute("aria-label")).toBe("Buffering chapter First Chapter");

    cleanup?.(); cleanup = undefined; document.body.innerHTML = "";
    const wrongSource = render({
      tracks: [duplicate], currentIndex: 0, chaptersByTrack: { [duplicate.id]: [chapter] },
      chapterRangesByTrack: { [duplicate.id]: { [chapter.id]: { ...range, sourceUrl: "https://other.test/audio" } } },
      activePlaybackTrack, buffering: true,
    });
    wrongSource.querySelector<HTMLElement>(".queue-list article summary")?.click(); await tick();
    [...wrongSource.querySelectorAll<HTMLButtonElement>(".queue-more-menu button")].find(button => button.textContent === "Show chapters")?.click();
    await tick();
    expect(wrongSource.querySelector(".chapter-child")?.classList.contains("current")).toBe(false);
  });

  it("uses native excerpt position as an absolute source position for current-chapter selection", async () => {
    const chapter = { id: "native-candidate-chapter", title: "Middle section", startTime: 110, endTime: 130, confidence: "high" };
    const playbackRange = { parentTrackId: track.id, chapterId: chapter.id, sourceUrl: "https://source.test/audio", startTime: 100, endTime: 160, parentTitle: track.title };
    const rangedTrack = { ...track, id: `chapter:${track.id}:${chapter.id}`, playbackRange };
    const target = render({
      tracks: [rangedTrack], currentIndex: 0, chapters: [chapter], position: 20, chapterPosition: 120,
      activePlaybackTrack: rangedTrack,
    });
    const chapterButton = target.querySelector<HTMLButtonElement>(".chapter-list .chapter-play")!;
    expect(chapterButton.getAttribute("aria-current")).toBe("time");
    expect(chapterButton.getAttribute("aria-label")).toBe("Playing chapter Middle section");
    expect(chapterButton.textContent).toContain("♫");
  });

  it("keeps paused state tied to the selected queue occurrence", async () => {
    const target = render({ tracks: [track, { ...track, id: "queue-2" }], currentIndex: 1, paused: true });
    const rows = [...target.querySelectorAll<HTMLElement>(".queue-list article")];
    expect(rows.map(row => row.classList.contains("paused"))).toEqual([false, true]);
    expect(rows.map(row => row.classList.contains("current"))).toEqual([false, true]);
    expect(rows.map(row => row.querySelector(".track-index")?.textContent)).toEqual(["01", "Ⅱ"]);
    expect(rows[1].querySelector(".track-select")?.getAttribute("aria-label")).toBe("Paused Queued Track");
  });

  it("keeps chapter rows to play and More, with queue, download, and info in the menu", async () => {
    const chapter = { id: "chapter-1", title: "First Chapter", startTime: 0, endTime: 120, confidence: "high" };
    const onQueueChapter = vi.fn(), onDownloadChapter = vi.fn(), onInfoChapter = vi.fn();
    const target = render({
      chapters: [chapter], onQueueChapter, onDownloadChapter, onInfoChapter,
    });
    const row = target.querySelector(".chapter-row")!;
    expect(row.querySelectorAll(":scope > button")).toHaveLength(1);
    const more = row.querySelector<HTMLElement>("summary")!;
    expect(more.getAttribute("aria-label")).toBe("More actions for chapter First Chapter");
    more.click(); await tick();
    [...more.closest("details")!.querySelectorAll<HTMLButtonElement>('.queue-more-menu button')].find(button => button.textContent?.trim() === "Queue chapter")!.click();
    expect(onQueueChapter).toHaveBeenCalledWith(track, chapter, 0);
  });
});

describe("queue occurrence selection and movement",()=>{
  it("Ctrl and Shift selects without playback, then serializes keyboard movement",async()=>{
    const onPlay=vi.fn(),onMove=vi.fn(async()=>{});
    const target=render({tracks:[track,{...track}, {...track,id:"third"},{...track,id:"fourth"}],onPlay,onMove});await tick();
    const play=target.querySelectorAll<HTMLButtonElement>(".track-select");
    play[1]!.dispatchEvent(new MouseEvent("click",{bubbles:true,ctrlKey:true}));await tick();
    play[2]!.dispatchEvent(new MouseEvent("click",{bubbles:true,shiftKey:true}));await tick();
    expect(onPlay).not.toHaveBeenCalled();
    expect(target.querySelectorAll('[aria-pressed="true"].queue-drag-select')).toHaveLength(2);
    target.querySelectorAll<HTMLButtonElement>(".queue-drag-select")[1]!.dispatchEvent(new KeyboardEvent("keydown",{key:"ArrowUp",bubbles:true,cancelable:true}));
    for(let i=0;i<10;i++)await Promise.resolve();await tick();
    expect(onMove.mock.calls).toEqual([[1,0],[2,1]]);
    expect(target.querySelectorAll<HTMLButtonElement>(".queue-drag-select")[0]!.getAttribute("aria-pressed")).toBe("true");
    expect(onPlay).not.toHaveBeenCalled();
  });
  it("waits for each reorder acknowledgement and ignores repeated keys while busy",async()=>{
    let finish!:()=>void;
    const onMove=vi.fn().mockImplementationOnce(()=>new Promise<void>(resolve=>{finish=resolve;})).mockResolvedValue(undefined);
    const target=render({tracks:[track,{...track},{...track,id:"third"}],onMove});await tick();
    const grips=target.querySelectorAll<HTMLButtonElement>(".queue-drag-select");
    grips[1]!.click();await tick();grips[2]!.dispatchEvent(new MouseEvent("click",{bubbles:true,shiftKey:true}));await tick();
    grips[1]!.dispatchEvent(new KeyboardEvent("keydown",{key:"ArrowUp",bubbles:true,cancelable:true}));await tick();
    expect(onMove.mock.calls).toEqual([[1,0]]);
    grips[1]!.dispatchEvent(new KeyboardEvent("keydown",{key:"ArrowUp",bubbles:true,cancelable:true}));await tick();
    expect(onMove).toHaveBeenCalledTimes(1);
    finish();for(let i=0;i<10;i++)await Promise.resolve();await tick();
    expect(onMove.mock.calls).toEqual([[1,0],[2,1]]);
  });
  it("dragging a duplicate occurrence moves only that occurrence",async()=>{
    const onMove=vi.fn(),onPlay=vi.fn();const target=render({tracks:[track,{...track},{...track,id:"third"}],onMove,onPlay});await tick();
    const grip=target.querySelectorAll<HTMLButtonElement>(".queue-drag-select")[1]!;
    grip.dispatchEvent(new Event("dragstart",{bubbles:true,cancelable:true}));await tick();
    target.querySelectorAll(".queue-list article")[0]!.dispatchEvent(new Event("drop",{bubbles:true,cancelable:true}));
    for(let i=0;i<10;i++)await Promise.resolve();await tick();
    expect(onMove).toHaveBeenCalledExactlyOnceWith(1,0);expect(onPlay).not.toHaveBeenCalled();
  });
});


describe("queue keyboard focus after reorder",()=>{
  it("keeps consecutive arrows on the selected duplicate occurrence",async()=>{
    const onMove=vi.fn(async()=>{}),onPlay=vi.fn();
    const target=render({tracks:[track,track,{...track,id:"third"}],onMove,onPlay});await tick();
    const grip=target.querySelectorAll<HTMLButtonElement>(".queue-drag-select")[1]!;
    grip.click();grip.focus();await tick();
    grip.dispatchEvent(new KeyboardEvent("keydown",{key:"ArrowUp",bubbles:true,cancelable:true}));
    for(let i=0;i<10;i++)await Promise.resolve();await tick();
    expect(document.activeElement).toBe(target.querySelectorAll(".queue-drag-select")[0]);
    document.activeElement!.dispatchEvent(new KeyboardEvent("keydown",{key:"ArrowDown",bubbles:true,cancelable:true}));
    for(let i=0;i<10;i++)await Promise.resolve();await tick();
    expect(onMove.mock.calls).toEqual([[1,0],[1,0]]);
    expect(document.activeElement).toBe(target.querySelectorAll(".queue-drag-select")[1]);
    expect(onPlay).not.toHaveBeenCalled();
  });
  it("stops a failed batch and permits a new selection without playback",async()=>{
    const onMove=vi.fn().mockRejectedValueOnce(new Error("fixture")).mockResolvedValue(undefined),onPlay=vi.fn();
    const target=render({tracks:[track,track,{...track,id:"third"}],onMove,onPlay});await tick();
    const grips=target.querySelectorAll<HTMLButtonElement>(".queue-drag-select");
    grips[1]!.click();grips[2]!.dispatchEvent(new MouseEvent("click",{bubbles:true,shiftKey:true}));await tick();
    grips[1]!.dispatchEvent(new KeyboardEvent("keydown",{key:"ArrowUp",bubbles:true,cancelable:true}));
    for(let i=0;i<10;i++)await Promise.resolve();await tick();
    expect(onMove).toHaveBeenCalledTimes(1);
    expect(target.textContent).toContain("Could not finish moving queue tracks.");
    expect(target.querySelectorAll('.queue-drag-select[aria-pressed="true"]')).toHaveLength(0);
    grips[1]!.click();grips[1]!.dispatchEvent(new KeyboardEvent("keydown",{key:"ArrowUp",bubbles:true,cancelable:true}));
    for(let i=0;i<10;i++)await Promise.resolve();await tick();
    expect(onMove).toHaveBeenCalledTimes(2);expect(onPlay).not.toHaveBeenCalled();
  });
});


it("chains native revisions within one normal selected-block gesture",async()=>{
 const onMove=vi.fn().mockResolvedValueOnce({queueRevision:11}).mockResolvedValueOnce({queueRevision:12});
 const target=render({tracks:[track,track,track,track],queueRevision:10,onMove});await tick();
 const grips=target.querySelectorAll<HTMLButtonElement>('.queue-drag-select');grips[1]!.click();grips[2]!.dispatchEvent(new MouseEvent('click',{bubbles:true,shiftKey:true}));
 grips[1]!.dispatchEvent(new KeyboardEvent('keydown',{key:'ArrowUp',bubbles:true,cancelable:true}));
 for(let i=0;i<12;i++)await Promise.resolve();await tick();
 expect(onMove.mock.calls.map(call=>call[2])).toEqual([10,11]);expect(target.querySelector('[role="alert"]')).toBeNull();
});
it("stops a normal selected-block move on stale or missing native acknowledgement",async()=>{
 for(const acknowledgement of [undefined,{queueRevision:19}]){
  const onMove=vi.fn().mockResolvedValue(acknowledgement);const target=render({tracks:[track,track,track,track],queueRevision:10,onMove});await tick();
  const grips=target.querySelectorAll<HTMLButtonElement>('.queue-drag-select');grips[1]!.click();grips[2]!.dispatchEvent(new MouseEvent('click',{bubbles:true,shiftKey:true}));
  grips[1]!.dispatchEvent(new KeyboardEvent('keydown',{key:'ArrowUp',bubbles:true,cancelable:true}));for(let i=0;i<12;i++)await Promise.resolve();await tick();
  expect(onMove).toHaveBeenCalledTimes(1);expect(onMove.mock.calls[0]![2]).toBe(10);expect(target.querySelector('[role="alert"]')?.textContent).toContain('Could not finish');cleanup?.();cleanup=undefined;document.body.innerHTML='';
 }
});
it("uses captured native revision for overflow moves and surfaces an external stale rejection",async()=>{
 const onMove=vi.fn().mockRejectedValue(new Error('Queue revision changed before reorder'));const target=render({tracks:[track,track],queueRevision:10,onMove});await tick();
 const row=target.querySelectorAll<HTMLElement>('.queue-list article')[1]!;row.querySelector<HTMLElement>('summary')!.click();await tick();
 const buttons=row.querySelectorAll<HTMLButtonElement>('.queue-more-menu button');[...buttons].find(button=>button.textContent?.trim()==='Move up')!.click();for(let i=0;i<12;i++)await Promise.resolve();await tick();
 expect(onMove).toHaveBeenCalledExactlyOnceWith(1,0,10);expect(target.querySelector('[role="alert"]')?.textContent).toContain('Could not finish');
});

it("does not adopt an external revision between normal batch steps",async()=>{
 let nativeRevision=10;const onMove=vi.fn(async(_from:number,_to:number,expected:number)=>{if(expected!==nativeRevision)throw new Error('stale');nativeRevision++;const own={queueRevision:nativeRevision};nativeRevision++;return own;});
 const target=render({tracks:[track,track,track,track],queueRevision:10,onMove});await tick();const grips=target.querySelectorAll<HTMLButtonElement>('.queue-drag-select');grips[1]!.click();grips[2]!.dispatchEvent(new MouseEvent('click',{bubbles:true,shiftKey:true}));grips[1]!.dispatchEvent(new KeyboardEvent('keydown',{key:'ArrowUp',bubbles:true,cancelable:true}));for(let i=0;i<12;i++)await Promise.resolve();await tick();
 expect(onMove.mock.calls.map(call=>call[2])).toEqual([10,11]);expect(nativeRevision).toBe(12);expect(target.querySelector('[role="alert"]')?.textContent).toContain('Could not finish');
});

it("displays the chapter playable duration instead of the full parent duration",async()=>{
 const chapter={...track,durationSeconds:360,playbackRange:{parentTrackId:track.id,chapterId:'chapter',sourceUrl:track.sourceUrl,startTime:60,endTime:90,parentTitle:track.title}};
 const target=render({tracks:[chapter,{...track,durationSeconds:null},{...chapter,playbackRange:{...chapter.playbackRange,endTime:60}}]});await tick();
 const rows=target.querySelectorAll('.queue-list > article');expect(rows[0]!.querySelector('.track-duration')?.textContent).toBe('0:30');expect(rows[1]!.querySelector('.track-duration')).toBeNull();expect(rows[2]!.querySelector('.track-duration')).toBeNull();
});
