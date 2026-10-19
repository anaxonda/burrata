// @vitest-environment jsdom
// @ts-expect-error Exercise Svelte's browser runtime in jsdom.
import { mount, tick, unmount } from "../node_modules/svelte/src/index-client.js";
import { afterEach, describe, expect, it, vi } from "vitest";
// @ts-expect-error Match the browser runtime used by the direct import above.
vi.mock("svelte", async () => await import("../node_modules/svelte/src/index-client.js"));
import TrackMenu from "./TrackMenu.svelte";
import { SOURCE_OPENER } from "./lib/source-opening";
import type { Track } from "./lib/types";

const track: Track = { id: "t1", provider: "library", providerId: "t1", sourceUrl: "", artist: "Artist", title: "Song", favorite: false, localPath: "/music/song.flac", provenance: {} };
let destroy: (() => void) | undefined;
afterEach(() => { destroy?.(); destroy = undefined; document.body.innerHTML = ""; vi.restoreAllMocks(); });

describe("TrackMenu", () => {
  it("keeps playback actions before library actions to match TrackList overflow", async () => {
    globalThis.ResizeObserver = class { observe() {} unobserve() {} disconnect() {} } as unknown as typeof ResizeObserver;
    Object.defineProperty(HTMLDialogElement.prototype, "showModal", { configurable: true, value: function (this: HTMLDialogElement) { this.setAttribute("open", ""); } });
    const noop = vi.fn();
    const instance = mount(TrackMenu, { target: document.body, props: {
      track, onClose: noop, onNext: noop, onQueue: noop, onPlaylist: noop, onFavorite: noop,
      onDownload: noop, onDetails: noop, onSelect: noop,
    } as never });
    destroy = () => unmount(instance); await tick();
    const labels = [...document.querySelectorAll<HTMLButtonElement>(".track-menu > button")].map(button => button.textContent?.trim());
    expect(labels.slice(0, 4)).toEqual(["Play next", "Add to queue", "Add to playlist…", "Add to Liked Songs"]);
  });

  it("shows downloaded availability without duplicating the download action", async () => {
    globalThis.ResizeObserver = class { observe() {} unobserve() {} disconnect() {} } as unknown as typeof ResizeObserver;
    Object.defineProperty(HTMLDialogElement.prototype, "showModal", { configurable: true, value: function (this: HTMLDialogElement) { this.setAttribute("open", ""); } });
    const onUpgrade = vi.fn(), onDownload = vi.fn(), onClose = vi.fn(), noop = vi.fn();
    const instance = mount(TrackMenu, { target: document.body, props: {
      track: { ...track, localPath: null, localAvailability: "available" }, onUpgrade, onClose, onNext: noop, onQueue: noop,
      onPlaylist: noop, onFavorite: noop, onDownload, onDetails: noop, onSelect: noop,
    } as never });
    destroy = () => unmount(instance);
    await tick();
    expect(document.activeElement).toBe(document.querySelector(".track-menu .primary-action"));
    expect(document.body.textContent).toContain("Downloaded");
    expect(document.body.textContent).not.toContain("Find higher-quality download");
    const download = [...document.querySelectorAll<HTMLButtonElement>("button")].find(button => button.textContent?.includes("Download options"));
    expect(download).toBeTruthy();
    expect(document.querySelector(".track-menu .primary-action")?.textContent).toContain("Add to playlist");
    download!.click(); await tick();
    expect(onDownload).toHaveBeenCalledOnce();
    expect(onUpgrade).not.toHaveBeenCalled();
    expect(onClose).toHaveBeenCalledOnce();
  });

  it("omits the empty artist line when track metadata is incomplete", async () => {
    globalThis.ResizeObserver = class { observe() {} unobserve() {} disconnect() {} } as unknown as typeof ResizeObserver;
    Object.defineProperty(HTMLDialogElement.prototype, "showModal", { configurable: true, value: function (this: HTMLDialogElement) { this.setAttribute("open", ""); } });
    const instance = mount(TrackMenu, { target: document.body, props: {
      track: { ...track, artist: "", title: "A long recording title without artist metadata" }, onClose: vi.fn(), onNext: vi.fn(), onQueue: vi.fn(),
      onPlaylist: vi.fn(), onFavorite: vi.fn(), onDownload: vi.fn(), onDetails: vi.fn(), onSelect: vi.fn(),
    } as never });
    destroy = () => unmount(instance);
    await tick();
    expect(document.querySelector(".track-menu header strong")?.textContent).toBe("A long recording title without artist metadata");
    expect(document.querySelector(".track-menu header small")).toBeNull();
  });

  it("runs Add to playlist directly and closes without introducing a nested submenu", async () => {
    globalThis.ResizeObserver = class { observe() {} unobserve() {} disconnect() {} } as unknown as typeof ResizeObserver;
    Object.defineProperty(HTMLDialogElement.prototype, "showModal", { configurable: true, value: function (this: HTMLDialogElement) { this.setAttribute("open", ""); } });
    const onPlaylist=vi.fn(),onClose=vi.fn(),noop=vi.fn();
    const instance=mount(TrackMenu,{target:document.body,props:{track,onPlaylist,onClose,onNext:noop,onQueue:noop,onFavorite:noop,onDownload:noop,onDetails:noop,onSelect:noop} as never});
    destroy=()=>unmount(instance);await tick();
    const primary=document.querySelector<HTMLButtonElement>(".track-menu .primary-action")!;
    expect(primary.querySelector("button")).toBeNull();
    primary.click();await tick();
    expect(onPlaylist).toHaveBeenCalledOnce();
    expect(onClose).toHaveBeenCalledOnce();
  });

  it("offers an exact Go to album navigation action when the parent supplies a catalog target", async () => {
    globalThis.ResizeObserver = class { observe() {} unobserve() {} disconnect() {} } as unknown as typeof ResizeObserver;
    Object.defineProperty(HTMLDialogElement.prototype, "showModal", { configurable: true, value: function (this: HTMLDialogElement) { this.setAttribute("open", ""); } });
    const onGoToAlbum = vi.fn(), onClose = vi.fn(), noop = vi.fn();
    const instance = mount(TrackMenu, { target: document.body, props: {
      track: { ...track, album: "Selected edition" }, onGoToAlbum, onClose, onNext: noop, onQueue: noop,
      onPlaylist: noop, onFavorite: noop, onDownload: noop, onDetails: noop, onSelect: noop,
    } as never });
    destroy = () => unmount(instance); await tick();
    const albumAction = [...document.querySelectorAll<HTMLButtonElement>(".track-menu button")].find(button => button.textContent?.trim() === "Go to album")!;
    expect(albumAction).toBeTruthy();
    albumAction.click(); await tick();
    expect(onGoToAlbum).toHaveBeenCalledOnce();
    expect(onClose).toHaveBeenCalledOnce();
  });

  it("labels album navigation unavailable when a track has no exact identity", async () => {
    globalThis.ResizeObserver = class { observe() {} unobserve() {} disconnect() {} } as unknown as typeof ResizeObserver;
    Object.defineProperty(HTMLDialogElement.prototype, "showModal", { configurable: true, value: function (this: HTMLDialogElement) { this.setAttribute("open", ""); } });
    const instance = mount(TrackMenu, { target: document.body, props: {
      track: { ...track, album: "Unlinked album" }, onGoToAlbumUnavailable: true,
      onClose: vi.fn(), onNext: vi.fn(), onQueue: vi.fn(), onPlaylist: vi.fn(), onFavorite: vi.fn(), onDownload: vi.fn(), onDetails: vi.fn(), onSelect: vi.fn(),
    } as never });
    destroy = () => unmount(instance); await tick();
    const unavailable = [...document.querySelectorAll<HTMLButtonElement>(".track-menu button")].find(button => button.textContent?.trim() === "Go to album")!;
    expect(unavailable.disabled).toBe(true);
    expect(unavailable.title).toContain("Album link unavailable for this track");
    expect(unavailable.getAttribute("aria-label")).toContain("Album link unavailable for this track");
  });

  it("closes on Escape without bubbling app shortcuts", async () => {
    globalThis.ResizeObserver = class { observe() {} unobserve() {} disconnect() {} } as unknown as typeof ResizeObserver;
    Object.defineProperty(HTMLDialogElement.prototype, "showModal", { configurable: true, value: function (this: HTMLDialogElement) { this.setAttribute("open", ""); } });
    const onClose=vi.fn(),noop=vi.fn();
    const instance=mount(TrackMenu,{target:document.body,props:{track,onClose,onNext:noop,onQueue:noop,onPlaylist:noop,onFavorite:noop,onDownload:noop,onDetails:noop,onSelect:noop} as never});
    destroy=()=>unmount(instance);await tick();
    const menu=document.querySelector(".track-menu")!;
    const appShortcut=vi.fn();window.addEventListener("keydown",appShortcut);
    const escape=new KeyboardEvent("keydown",{key:"Escape",bubbles:true,cancelable:true});menu.dispatchEvent(escape);window.removeEventListener("keydown",appShortcut);
    expect(escape.defaultPrevented).toBe(true);
    expect(appShortcut).not.toHaveBeenCalled();
    expect(onClose).toHaveBeenCalledOnce();
  });

  it("wraps Tab at the visible focus boundaries and ignores disabled actions", async () => {
    globalThis.ResizeObserver = class { observe() {} unobserve() {} disconnect() {} } as unknown as typeof ResizeObserver;
    Object.defineProperty(HTMLDialogElement.prototype, "showModal", { configurable: true, value: function (this: HTMLDialogElement) { this.setAttribute("open", ""); } });
    const noop = vi.fn();
    const instance = mount(TrackMenu, { target: document.body, props: {
      track, onClose: vi.fn(), onNext: noop, onQueue: noop, onPlaylist: noop, onFavorite: noop,
      onDownload: noop, onDetails: noop, onSelect: noop, onRadio: noop, radioBusy: true,
    } as never });
    destroy = () => unmount(instance);
    await tick();
    const menu = document.querySelector(".track-menu")!;
    const buttons = [...menu.querySelectorAll<HTMLButtonElement>("button")];
    for (const button of buttons) button.getClientRects = () => { const rects = [new DOMRect(0, 0, 1, 1)]; return Object.assign(rects, { item: (index: number) => rects[index] ?? null }); };
    const disabled = buttons.find(button => button.disabled)!;
    disabled.tabIndex = 0;
    const excluded = document.createElement("button"); excluded.tabIndex = -1; menu.append(excluded); excluded.getClientRects = () => { const rects = [new DOMRect(0, 0, 1, 1)]; return Object.assign(rects, { item: (index: number) => rects[index] ?? null }); };
    const enabled = buttons.filter(button => !button.matches(":disabled") && button.tabIndex >= 0);
    const first = enabled[0], last = enabled[enabled.length - 1]!;
    expect(buttons.some(button => button.disabled)).toBe(true);
    last.focus();
    const forward = new KeyboardEvent("keydown", { key: "Tab", bubbles: true, cancelable: true });
    menu.dispatchEvent(forward);
    expect(forward.defaultPrevented).toBe(true);
    expect(document.activeElement).toBe(first);
    first.focus();
    const reverse = new KeyboardEvent("keydown", { key: "Tab", shiftKey: true, bubbles: true, cancelable: true });
    menu.dispatchEvent(reverse);
    expect(reverse.defaultPrevented).toBe(true);
    expect(document.activeElement).toBe(last);
    expect(document.activeElement).not.toBe(disabled);
  });

  it("omits the source divider when the track has no supported web source", async () => {
    globalThis.ResizeObserver = class { observe() {} unobserve() {} disconnect() {} } as unknown as typeof ResizeObserver;
    Object.defineProperty(HTMLDialogElement.prototype, "showModal", { configurable: true, value: function (this: HTMLDialogElement) { this.setAttribute("open", ""); } });
    const instance = mount(TrackMenu, { target: document.body, props: {
      track, onClose: vi.fn(), onNext: vi.fn(), onQueue: vi.fn(), onPlaylist: vi.fn(), onFavorite: vi.fn(), onDownload: vi.fn(), onDetails: vi.fn(), onSelect: vi.fn(),
    } as never });
    destroy = () => unmount(instance);
    await tick();
    expect(document.querySelector(".track-menu .source-row")).toBeNull();
  });

  it("closes after a source opens successfully and stays open when opening fails", async () => {
    globalThis.ResizeObserver = class { observe() {} unobserve() {} disconnect() {} } as unknown as typeof ResizeObserver;
    Object.defineProperty(HTMLDialogElement.prototype, "showModal", { configurable: true, value: function (this: HTMLDialogElement) { this.setAttribute("open", ""); } });
    const sourceTrack = { ...track, sourceUrl: "https://beatport.com/track/artist/song/123" };
    const onClose = vi.fn(), noop = vi.fn(), opener = vi.fn().mockResolvedValue(undefined);
    const instance = mount(TrackMenu, { target: document.body, props: {
      track: sourceTrack, onClose, onNext: noop, onQueue: noop, onPlaylist: noop, onFavorite: noop, onDownload: noop, onDetails: noop, onSelect: noop,
    } as never, context: new Map([[SOURCE_OPENER, opener]]) });
    destroy = () => unmount(instance);
    await tick();
    const openSource = document.querySelector<HTMLButtonElement>(".track-menu .source-row .source-link")!;
    openSource.click();
    await tick();
    expect(opener).toHaveBeenCalledWith(sourceTrack, "https://beatport.com/track/artist/song/123");
    expect(onClose).toHaveBeenCalledOnce();

    destroy(); destroy = undefined; document.body.innerHTML = "";
    opener.mockRejectedValueOnce(new Error("Source could not be opened"));
    const failedClose = vi.fn();
    const failedInstance = mount(TrackMenu, { target: document.body, props: {
      track: sourceTrack, onClose: failedClose, onNext: noop, onQueue: noop, onPlaylist: noop, onFavorite: noop, onDownload: noop, onDetails: noop, onSelect: noop,
    } as never, context: new Map([[SOURCE_OPENER, opener]]) });
    destroy = () => unmount(failedInstance);
    await tick();
    document.querySelector<HTMLButtonElement>(".track-menu .source-row .source-link")!.click();
    await new Promise(resolve => setTimeout(resolve, 0));
    await tick();
    expect(failedClose).not.toHaveBeenCalled();
    expect(document.querySelector(".track-menu")?.hasAttribute("open")).toBe(true);
    expect(document.querySelector('[role="alert"]')?.textContent).toContain("Source could not be opened");
  });
});

it("opens the exact ranged source once from its in-button icon or keyboard activation",async()=>{
 globalThis.ResizeObserver=class {observe(){}unobserve(){}disconnect(){}} as unknown as typeof ResizeObserver;
 Object.defineProperty(HTMLDialogElement.prototype,'showModal',{configurable:true,value:function(this:HTMLDialogElement){this.setAttribute('open','');}});
 const sourceTrack={...track,sourceUrl:'https://beatport.com/track/example/123',playbackRange:{parentTrackId:track.id,chapterId:'chapter-1',sourceUrl:'https://beatport.com/track/example/123',startTime:60,endTime:90,parentTitle:track.title}};
 const opener=vi.fn().mockResolvedValue(undefined),onClose=vi.fn(),noop=vi.fn();const before=JSON.stringify(sourceTrack);
 const instance=mount(TrackMenu,{target:document.body,props:{track:sourceTrack,onClose,onNext:noop,onQueue:noop,onPlaylist:noop,onFavorite:noop,onDownload:noop,onDetails:noop,onSelect:noop} as never,context:new Map([[SOURCE_OPENER,opener]])});destroy=()=>unmount(instance);await tick();
 const row=document.querySelector('.track-menu .source-row')!,button=row.querySelector<HTMLButtonElement>('.source-link')!,icon=button.querySelector('svg')!;
 expect(icon).toBeTruthy();expect(icon.getAttribute('aria-hidden')).toBe('true');expect(button.getAttribute('aria-label')).toBe('Open Song on Beatport');expect(row.querySelectorAll('button')).toHaveLength(1);
 icon.dispatchEvent(new MouseEvent('click',{bubbles:true,cancelable:true}));await tick();expect(opener).toHaveBeenCalledExactlyOnceWith(sourceTrack,sourceTrack.sourceUrl);expect(onClose).toHaveBeenCalledTimes(1);
 button.dispatchEvent(new MouseEvent('click',{bubbles:true,cancelable:true,detail:0}));await tick();expect(opener).toHaveBeenCalledTimes(2);expect(opener).toHaveBeenLastCalledWith(sourceTrack,sourceTrack.sourceUrl);expect(onClose).toHaveBeenCalledTimes(2);expect(JSON.stringify(sourceTrack)).toBe(before);
});

it("offers available standalone actions without inventing callbacks and navigates enabled actions with arrows", async () => {
  globalThis.ResizeObserver = class { observe() {} unobserve() {} disconnect() {} } as unknown as typeof ResizeObserver;
  Object.defineProperty(HTMLDialogElement.prototype, "showModal", { configurable: true, value(this: HTMLDialogElement) { this.setAttribute("open", ""); } });
  const onArtist = vi.fn(), onClose = vi.fn();
  const instance = mount(TrackMenu, { target: document.body, props: { track, onArtist, onClose, onGoToAlbumUnavailable: true } });
  destroy = () => unmount(instance); await tick();
  const menu = document.querySelector<HTMLDialogElement>(".action-menu.track-menu")!;
  const artist = [...menu.querySelectorAll<HTMLButtonElement>("button")].find(button => button.textContent?.trim() === "Open artist")!;
  for (const button of menu.querySelectorAll<HTMLButtonElement>("button")) button.getClientRects = () => { const rects = [new DOMRect(0, 0, 1, 1)]; return Object.assign(rects, { item: (index: number) => rects[index] ?? null }); };
  expect(menu.textContent).not.toContain("Play next");
  expect(menu.textContent).not.toContain("Add to playlist");
  menu.querySelector<HTMLButtonElement>("header button")!.focus();
  const arrow = new KeyboardEvent("keydown", { key: "ArrowDown", bubbles: true, cancelable: true });
  menu.dispatchEvent(arrow);
  expect(arrow.defaultPrevented).toBe(true);
  expect(document.activeElement).toBe(artist);
  artist.click(); await tick();
  expect(onArtist).toHaveBeenCalledOnce();
  expect(onClose).toHaveBeenCalledOnce();
});
