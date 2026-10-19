// @vitest-environment jsdom
// @ts-expect-error Exercise Svelte's browser runtime in jsdom.
import { mount, tick, unmount } from "../node_modules/svelte/src/index-client.js";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
// @ts-expect-error Svelte internal client runtime lacks public declarations
vi.mock("svelte", async () => await import("../node_modules/svelte/src/index-client.js"));
import ReleaseView from "./ReleaseView.svelte";
import ReleaseViewHarness from "./ReleaseViewHarness.svelte";
import type { ArtistReleaseRef } from "./lib/artist-types";
import type { Track } from "./lib/types";

beforeEach(() => {
  globalThis.ResizeObserver = class {observe() {} unobserve() {} disconnect() {}} as unknown as typeof ResizeObserver;
  Object.defineProperty(HTMLDialogElement.prototype, "showModal", {configurable: true, value: function(this: HTMLDialogElement) {this.setAttribute("open", "");}});
});

const release: ArtistReleaseRef = {
  release: { provider: "musicbrainz", id: "release-1" }, title: "A Great Album", year: 2020, releaseDate: "2020-06-01", label: "Example Records", primaryType: "Album", trackCount: 2,
  sourceUrl: "https://musicbrainz.org/release/release-1", tracks: [], credits: [{ name: "The Example", role: "artist", sourceUrl: "https://musicbrainz.org/artist/example" }],
};
const tracks: Track[] = [
  { id: "t1", provider: "musicbrainz", providerId: "t1", sourceUrl: "https://example.com/1", artist: "The Example", title: "Zulu", durationSeconds: 180, favorite: false, provenance: { position: "2" } },
  { id: "t2", provider: "musicbrainz", providerId: "t2", sourceUrl: "https://example.com/2", artist: "The Example", title: "Alpha", durationSeconds: 120, favorite: false, provenance: { position: "1" } },
];
let destroy: (() => void) | undefined;
afterEach(() => { destroy?.(); destroy = undefined; document.body.innerHTML = ""; vi.restoreAllMocks(); });
async function settle() { for (let i = 0; i < 8; i++) await Promise.resolve(); await tick(); }
function render(props: Record<string, unknown> = {}) {
  const target = document.createElement("div"); document.body.append(target);
  const instance = mount(ReleaseView, { target, props: { release, tracks, ...props } as never });
  destroy = () => unmount(instance);
  return target;
}

describe("ReleaseView", () => {
  it("plays and queues the album, with track playback using the displayed list and index", async () => {
    const onPlay = vi.fn(); const onQueue = vi.fn();
    const target = render({ onPlay, onQueue });
    target.querySelector<HTMLButtonElement>(".hero-actions .primary")!.click();
    target.querySelector<HTMLButtonElement>(".hero-actions button:not(.primary)")!.click();
    expect(onPlay).toHaveBeenCalledWith(tracks[1], [tracks[1], tracks[0]], 0);
    expect(onQueue).toHaveBeenCalledWith([tracks[1], tracks[0]], "append");
    target.querySelector<HTMLButtonElement>('[aria-label="Play Zulu"]')!.click();
    expect(onPlay).toHaveBeenLastCalledWith(tracks[0], [tracks[1], tracks[0]], 1);
  });

  it("saves the full loaded album in track-number order, independent of filtering", async () => {
    const onSaveToLibrary = vi.fn(async () => {});
    const target = render({ onSaveToLibrary });
    expect(target.querySelector<HTMLButtonElement>(".save-album")?.textContent).toContain("Save album to Library");
    const filter = target.querySelector<HTMLInputElement>("[aria-label='Filter tracks']")!;
    filter.value = "Zulu"; filter.dispatchEvent(new Event("input", { bubbles: true })); await settle();
    target.querySelector<HTMLButtonElement>(".save-album")!.click(); await settle();
    expect(onSaveToLibrary).toHaveBeenCalledOnce();
    expect(onSaveToLibrary).toHaveBeenCalledWith([tracks[1], tracks[0]]);
    expect(target.querySelector('[role="status"]')?.textContent).toContain("Album tracks saved to Library.");
  });

  it("labels a partial release save with the loaded and expected counts", async () => {
    const partialRelease = { ...release, trackCount: 5 };
    const onSaveToLibrary = vi.fn(async () => {});
    const target = render({ release: partialRelease, onSaveToLibrary });
    expect(target.querySelector<HTMLButtonElement>(".save-album")?.textContent).toContain("Save loaded tracks (2 of 5) to Library");
  });

  it("keeps unknown-count and duplicate-ID saves honest", async () => {
    const unknownRelease = { ...release, trackCount: null };
    const onSaveToLibrary = vi.fn(async () => {});
    const repeated = { ...tracks[0], provenance: { position: "1" } };
    const target = render({ release: unknownRelease, tracks: [repeated, repeated], onSaveToLibrary });
    expect(target.querySelector<HTMLButtonElement>(".save-album")?.textContent).toContain("Save loaded tracks (2) to Library");
    target.querySelector<HTMLButtonElement>(".save-album")!.click(); await settle();
    expect(onSaveToLibrary).toHaveBeenCalledWith([repeated, repeated]);
    expect(target.querySelector('[role="status"]')?.textContent).toBe("Album tracks saved to Library.");
    expect(target.querySelector('[role="status"]')?.textContent).not.toMatch(/\b2\b/);
  });

  it("ignores a pending save result after navigating to another release", async () => {
    let finishSave!: () => void;
    const onSaveToLibrary = vi.fn(() => new Promise<void>((resolve) => { finishSave = resolve; }));
    const target = document.createElement("div"); document.body.append(target);
    const instance = mount(ReleaseViewHarness, { target, props: { release, tracks, onSaveToLibrary } }) as unknown as { setRelease: (next: ArtistReleaseRef) => void };
    destroy = () => unmount(instance);
    await settle();
    target.querySelector<HTMLButtonElement>(".save-album")!.click(); await settle();
    expect(target.querySelector<HTMLButtonElement>(".save-album")?.disabled).toBe(true);
    instance.setRelease({ ...release, release: { provider: "musicbrainz", id: "release-2" }, title: "Different Album", sourceUrl: "https://musicbrainz.org/release/release-2" });
    await settle();
    expect(target.querySelector<HTMLButtonElement>(".save-album")?.disabled).toBe(false);
    finishSave(); await settle();
    expect(target.querySelector('[role="status"]')).toBeNull();
    expect(target.querySelector('[role="alert"]')).toBeNull();
  });

  it("shows save failures and disables repeated saves while the callback is pending", async () => {
    let rejectSave!: (reason: Error) => void;
    const onSaveToLibrary = vi.fn(() => new Promise<void>((_resolve, reject) => { rejectSave = reject; }));
    const target = render({ onSaveToLibrary });
    const button = target.querySelector<HTMLButtonElement>(".save-album")!;
    button.click(); await settle();
    expect(button.disabled).toBe(true);
    rejectSave(new Error("Library write failed")); await settle();
    expect(button.disabled).toBe(false);
    expect(target.querySelector('[role="alert"]')?.textContent).toContain("Library write failed");
  });

  it("offers a whole-album download using track-number order", async () => {
    const onDownloadAlbum = vi.fn();
    const target = render({ onDownloadAlbum });
    target.querySelector<HTMLButtonElement>(".hero-actions button:last-child")!.click();
    expect(onDownloadAlbum).toHaveBeenCalledWith([tracks[1], tracks[0]]);
  });

  it("filters and sorts track rows, and wires row actions while leaving navigation to the app toolbar", async () => {
    const onDownload = vi.fn(); const onArtist = vi.fn(); const onPlaylist = vi.fn(); const onContext = vi.fn();
    const target = render({ onDownload, onArtist, onPlaylist, onContext });
    const sort = target.querySelector<HTMLSelectElement>('[aria-label="Sort tracks"]')!;
    sort.value = "title"; sort.dispatchEvent(new Event("change", { bubbles: true })); await settle();
    expect([...target.querySelectorAll<HTMLButtonElement>(".track-title")].map(button => button.textContent?.trim())).toEqual(["Alpha", "Zulu"]);
    target.querySelector<HTMLButtonElement>('[aria-label="Download Alpha"]')!.click();
    target.querySelector<HTMLButtonElement>('[aria-label="Actions for Alpha"]')!.click();
    expect(onContext).toHaveBeenCalledWith(tracks[1], expect.any(MouseEvent));
    expect(target.querySelector(".track-menu")).toBeNull();
    target.querySelector<HTMLButtonElement>(".artist-link")!.click();
    const row = target.querySelector(".shared-track-list li")!;
    const contextEvent = new MouseEvent("contextmenu", { bubbles: true, cancelable: true }); row.dispatchEvent(contextEvent);
    expect(target.querySelector(".back")).toBeNull();
    expect(onDownload).toHaveBeenCalledWith(tracks[1]);
    expect(onPlaylist).not.toHaveBeenCalled();
    expect(onArtist).toHaveBeenCalledWith(tracks[1]);
    expect(onContext).toHaveBeenCalledWith(tracks[1], expect.any(MouseEvent));
    expect(contextEvent.defaultPrevented).toBe(true);
    const filter = target.querySelector<HTMLInputElement>('[aria-label="Filter tracks"]')!;
    filter.value = "Zulu"; filter.dispatchEvent(new Event("input", { bubbles: true })); await settle();
    expect(target.querySelectorAll(".shared-track-list li")).toHaveLength(1);
    expect(target.querySelector(".track-title")?.textContent).toContain("Zulu");
  });

  it("applies duration bounds to the release rows without changing album order", async () => {
    const target = render({ playingTrack: { ...tracks[0], title: "Updated title" }, playingTrackId: "stale-id" });
    expect(target.querySelectorAll(".current")).toHaveLength(1);
    const min = target.querySelector<HTMLInputElement>('[aria-label="Minimum track duration"]')!;
    min.value = "2:30"; min.dispatchEvent(new Event("input", { bubbles: true })); await settle();
    expect(target.querySelectorAll(".shared-track-list li")).toHaveLength(1);
    expect(target.querySelector(".track-title")?.textContent).toContain("Zulu");
    expect(target.querySelector(".current")).not.toBeNull();
    const sort = target.querySelector<HTMLSelectElement>('[aria-label="Sort tracks"]')!;
    sort.value = "sourceOrder"; sort.dispatchEvent(new Event("change", { bubbles: true })); await settle();
    expect(target.querySelector(".track-title")?.textContent).toContain("Zulu");
    min.value = ""; min.dispatchEvent(new Event("input", { bubbles: true })); await settle();
    const direction = target.querySelector<HTMLSelectElement>('[aria-label="Sort direction"]')!;
    direction.value = "desc"; direction.dispatchEvent(new Event("change", { bubbles: true })); await settle();
    expect(target.querySelector(".track-title")?.textContent).toContain("Zulu");
  });

  it("keeps fallback positions for repeated track occurrences after sorting", async () => {
    const repeated: Track = { id: "repeated", provider: "musicbrainz", providerId: "repeated", sourceUrl: "https://example.com/repeated", artist: "The Example", title: "Zulu", favorite: false, provenance: {} };
    const alpha: Track = { id: "alpha", provider: "musicbrainz", providerId: "alpha", sourceUrl: "https://example.com/alpha", artist: "The Example", title: "Alpha", favorite: false, provenance: {} };
    const onPlay = vi.fn();
    const target = render({ tracks: [repeated, alpha, repeated], onPlay });
    const sort = target.querySelector<HTMLSelectElement>('[aria-label="Sort tracks"]')!;
    sort.value = "title"; sort.dispatchEvent(new Event("change", { bubbles: true })); await settle();
    expect([...target.querySelectorAll<HTMLElement>(".track-position")].map(node => node.textContent?.trim())).toEqual(["2", "1", "3"]);
    sort.value = "sourceOrder"; sort.dispatchEvent(new Event("change", { bubbles: true })); await settle();
    const direction = target.querySelector<HTMLSelectElement>('[aria-label="Sort direction"]')!;
    direction.value = "desc"; direction.dispatchEvent(new Event("change", { bubbles: true })); await settle();
    expect([...target.querySelectorAll<HTMLElement>(".track-position")].map(node => node.textContent?.trim())).toEqual(["3", "2", "1"]);
    target.querySelector<HTMLButtonElement>('[aria-label="Play Zulu"]')!.click();
    expect(onPlay).toHaveBeenCalledWith(repeated, [repeated, alpha, repeated], 0);
  });

  it("adds selected repeated album occurrences without collapsing their identity", async () => {
    const repeated: Track = { ...tracks[0]!, provenance: { remoteEntryId: "album-entry-a" } };
    const third: Track = { ...tracks[0]!, provenance: { remoteEntryId: "album-entry-b" } };
    const onAddSelectedToPlaylist = vi.fn();
    const target = render({ tracks: [repeated, tracks[1]!, third], onAddSelectedToPlaylist });
    const sort = target.querySelector<HTMLSelectElement>('[aria-label="Sort tracks"]')!;
    sort.value = "title"; sort.dispatchEvent(new Event("change", { bubbles: true })); await settle();
    target.querySelector<HTMLButtonElement>('.start-selection')!.click(); await settle();
    const checks = [...target.querySelectorAll<HTMLInputElement>('[aria-label="Select Zulu for playlist"]')];
    checks[0]!.click(); checks[1]!.click();
    await settle();
    sort.value = "sourceOrder"; sort.dispatchEvent(new Event("change", { bubbles: true })); await settle();
    target.querySelector<HTMLButtonElement>(".add-selected")!.click();
    expect(onAddSelectedToPlaylist).toHaveBeenCalledWith([repeated, third]);
  });
});
