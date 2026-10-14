// @vitest-environment jsdom
// @ts-expect-error Exercise Svelte's browser runtime in jsdom.
import { mount, tick, unmount } from "../node_modules/svelte/src/index-client.js";
import { afterEach, describe, expect, it, vi } from "vitest";
// @ts-expect-error Svelte internal client runtime lacks public declarations
vi.mock("svelte", async () => await import("../node_modules/svelte/src/index-client.js"));
import ReleaseView from "./ReleaseView.svelte";
import type { ArtistReleaseRef } from "./lib/artist-types";
import type { Track } from "./lib/types";

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

  it("offers a whole-album download using track-number order", async () => {
    const onDownloadAlbum = vi.fn();
    const target = render({ onDownloadAlbum });
    target.querySelector<HTMLButtonElement>(".hero-actions button:last-child")!.click();
    expect(onDownloadAlbum).toHaveBeenCalledWith([tracks[1], tracks[0]]);
  });

  it("filters and sorts track rows, and wires download, artist, playlist, context, and back actions", async () => {
    const onDownload = vi.fn(); const onArtist = vi.fn(); const onPlaylist = vi.fn(); const onContext = vi.fn(); const onBack = vi.fn();
    const target = render({ onDownload, onArtist, onPlaylist, onContext, onBack });
    const sort = target.querySelector<HTMLSelectElement>('[aria-label="Sort tracks"]')!;
    sort.value = "title"; sort.dispatchEvent(new Event("change", { bubbles: true })); await settle();
    expect([...target.querySelectorAll<HTMLButtonElement>(".track-title")].map(button => button.textContent?.trim())).toEqual(["Alpha", "Zulu"]);
    target.querySelector<HTMLButtonElement>('[aria-label="Download Alpha"]')!.click();
    target.querySelector<HTMLButtonElement>('[aria-label="More actions for Alpha"]')!.click();
    [...target.querySelectorAll<HTMLButtonElement>(".more-menu button")].find(button => button.textContent === "Add to playlist")!.click();
    target.querySelector<HTMLButtonElement>(".artist-link")!.click();
    const row = target.querySelector(".track-list li")!;
    const contextEvent = new MouseEvent("contextmenu", { bubbles: true, cancelable: true }); row.dispatchEvent(contextEvent);
    target.querySelector<HTMLButtonElement>(".back")!.click();
    expect(onDownload).toHaveBeenCalledWith(tracks[1]);
    expect(onPlaylist).toHaveBeenCalledWith(tracks[1]);
    expect(onArtist).toHaveBeenCalledWith(tracks[1]);
    expect(onContext).toHaveBeenCalledWith(tracks[1], expect.any(MouseEvent));
    expect(contextEvent.defaultPrevented).toBe(true);
    expect(onBack).toHaveBeenCalledOnce();
    const filter = target.querySelector<HTMLInputElement>('[aria-label="Filter tracks"]')!;
    filter.value = "Zulu"; filter.dispatchEvent(new Event("input", { bubbles: true })); await settle();
    expect(target.querySelectorAll(".track-list li")).toHaveLength(1);
    expect(target.querySelector(".track-title")?.textContent).toContain("Zulu");
  });
});
