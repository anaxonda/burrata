// @vitest-environment jsdom
// @ts-expect-error Exercise Svelte's browser runtime in jsdom.
import { mount, tick, unmount } from "../node_modules/svelte/src/index-client.js";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
// @ts-expect-error Match the browser runtime used by the direct import above.
vi.mock("svelte", async () => await import("../node_modules/svelte/src/index-client.js"));
import SpotifyArtist from "./SpotifyArtist.svelte";
import { providerNavigationSnapshots } from "./lib/provider-navigation-snapshots";

let destroy: (() => void) | undefined;
beforeEach(() => {
  providerNavigationSnapshots.clear();
  vi.stubGlobal("requestAnimationFrame", (callback: FrameRequestCallback) => { callback(0); return 1; });
});
afterEach(() => { vi.unstubAllGlobals(); destroy?.(); destroy = undefined; document.body.innerHTML = ""; });
async function settle() { for (let i = 0; i < 10; i++) await Promise.resolve(); await tick(); }
function deferred<T = unknown>() {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>(done => { resolve = done; });
  return { promise, resolve };
}
const album = (id: string, title: string) => ({ id, title, url: `https://open.spotify.com/album/${id}`, artists: ["Fixture Artist"] });
const track = (id: string, title: string) => ({ id, title, artist: "Fixture Artist", album: "Release", durationSeconds: 180, source: "spotify" });
function render(call: (command: string, args: Record<string, unknown>) => Promise<unknown>, onPlay?: (track: unknown, list: unknown[], index: number) => void, artistId = "artist-id", extra: Record<string, unknown> = {}) {
  const target = document.createElement("div"); document.body.append(target);
  const instance = mount(SpotifyArtist, { target, props: { artistId, call, onPlay, ...extra } });
  destroy = () => unmount(instance);
  return target;
}

describe("SpotifyArtist pagination", () => {
  it("offers explicit MusicBrainz discovery for the exact Spotify artist name only after user activation", async () => {
    const id = "explicit-provider-search";
    const name = "Artist Name (Remastered)";
    const call = vi.fn(async (command: string) => {
      if (command === "browse_remote_artist") return {
        artist: { id, name, url: `https://open.spotify.com/artist/${id}`, genres: [] }, albums: [], topTracksUnavailable: "Unavailable",
      };
      if (command === "browse_remote_artist_tracks") return { provider: "spotify", artistId: id, tracks: [], total: 0 };
      throw new Error(`Unexpected command ${command}`);
    });
    const onBrowseArtistProvider = vi.fn();
    const target = render(call, undefined, id, { onBrowseArtistProvider });
    await settle();
    expect(onBrowseArtistProvider).not.toHaveBeenCalled();
    const browse = target.querySelector<HTMLButtonElement>(`[aria-label="Find ${name} on MusicBrainz"]`)!;
    expect(browse.textContent).toBe("Find on MusicBrainz");
    browse.click();
    expect(onBrowseArtistProvider).toHaveBeenCalledOnce();
    expect(onBrowseArtistProvider).toHaveBeenCalledWith(name, "musicbrainz");
    expect(target.querySelector(".hero h1")?.textContent).toBe(name);
    expect(call).not.toHaveBeenCalledWith("search_artist", expect.anything());
  });

  it("saves all loaded source-order album rows even when filtered, and labels an incomplete page honestly", async () => {
    const id = "save-loaded-artist";
    const release = album("partial-save", "Partial album");
    const first = { ...track("first", "First"), durationSeconds: 180 };
    const second = { ...track("second", "Second"), durationSeconds: 240 };
    const save = vi.fn(async (_tracks: unknown[]) => {});
    const call = vi.fn(async (command: string, args: Record<string, unknown>) => {
      if (command === "browse_remote_artist") return { artist: { id, name: "Save Artist", url: `https://open.spotify.com/artist/${id}`, genres: [] }, albums: [release] };
      if (command === "browse_remote_artist_tracks") return { provider: "spotify", artistId: id, tracks: [] };
      if (command === "browse_remote_album_tracks") return { album: release, tracks: args.cursor ? [] : [first, second], nextCursor: args.cursor ? null : "page-2", total: 3 };
      throw new Error(`Unexpected command ${command}`);
    });
    const target = render(call, undefined, id, { onSaveToLibrary: save }); await settle();
    target.querySelector<HTMLButtonElement>("button.album")!.click(); await settle();
    const minimum = target.querySelector<HTMLInputElement>('[aria-label="Minimum track duration"]')!;
    minimum.value = "200"; minimum.dispatchEvent(new Event("input", { bubbles: true })); await settle();
    expect(target.querySelectorAll(".shared-track-list li")).toHaveLength(1);
    const saveButton = target.querySelector<HTMLButtonElement>('[aria-label="Save loaded tracks to library"]')!;
    expect(saveButton.textContent).toBe("Save loaded tracks");
    saveButton.click(); await settle();
    expect(save).toHaveBeenCalledOnce();
    expect(save).toHaveBeenCalledWith([first, second]);
    expect(target.querySelector('[role="status"]')?.textContent).toContain("Saved 2 loaded tracks");
    expect(target.querySelector('[aria-label="Save album to library"]')).toBeNull();
  });

  it("does not show a save result from an old album after switching to another album", async () => {
    const id = "save-stale-artist";
    const first = album("save-one", "Save One");
    const second = album("save-two", "Save Two");
    const saving = deferred<void>();
    const save = vi.fn(() => saving.promise);
    const call = vi.fn(async (command: string, args: Record<string, unknown>) => {
      if (command === "browse_remote_artist") return { artist: { id, name: "Save Artist", url: `https://open.spotify.com/artist/${id}`, genres: [] }, albums: [first, second] };
      if (command === "browse_remote_artist_tracks") return { provider: "spotify", artistId: id, tracks: [] };
      if (command === "browse_remote_album_tracks") {
        const selected = args.albumId === first.id ? first : second;
        return { album: selected, tracks: [track(`${selected.id}-track`, `${selected.title} track`)], total: 1 };
      }
      throw new Error(`Unexpected command ${command}`);
    });
    const target = render(call, undefined, id, { onSaveToLibrary: save }); await settle();
    const albums = [...target.querySelectorAll<HTMLButtonElement>("button.album")];
    albums[0]!.click(); await settle();
    target.querySelector<HTMLButtonElement>('[aria-label="Save album to library"]')!.click(); await settle();
    expect(save).toHaveBeenCalledOnce();
    [...target.querySelectorAll<HTMLButtonElement>("button")].find(button => button.textContent === "Back to releases")!.click(); await settle();
    [...target.querySelectorAll<HTMLButtonElement>("button.album")].find(button => button.textContent?.includes("Save Two"))!.click(); await settle();
    expect(target.querySelector('[role="status"]')?.textContent ?? "").not.toContain("Saved album");
    saving.resolve(); await settle();
    expect(target.querySelector('[role="status"]')?.textContent ?? "").not.toContain("Saved album");
    expect(target.querySelector("h1")?.textContent).toContain("Save Two");
  });

  it("adds selected top-track occurrences to the local playlist picker", async () => {
    const id = "bulk-select-artist";
    const first = { ...track("same-track", "Same song"), provenance: { remoteEntryId: "top-entry-one" } };
    const second = { ...first, sourceUrl: "https://open.spotify.com/track/alternate", provenance: { remoteEntryId: "top-entry-two" } };
    const onAddSelectedToPlaylist = vi.fn();
    const call = vi.fn(async (command: string) => {
      if (command === "browse_remote_artist") return { artist: { id, name: "Bulk Artist", url: `https://open.spotify.com/artist/${id}`, genres: [] }, albums: [] };
      if (command === "browse_remote_artist_tracks") return { provider: "spotify", artistId: id, tracks: [first, second], total: 2 };
      throw new Error(`Unexpected command ${command}`);
    });
    const target = render(call, undefined, id, { onAddSelectedToPlaylist }); await settle();
    await vi.waitFor(() => expect(target.querySelector('.top-tracks [aria-label="Select tracks"]')).not.toBeNull());
    target.querySelector<HTMLButtonElement>('.top-tracks [aria-label="Select tracks"]')!.click();await settle();
    await vi.waitFor(() => expect(target.querySelectorAll('[aria-label="Select Same song for playlist"]')).toHaveLength(2));
    const checks = [...target.querySelectorAll<HTMLInputElement>('[aria-label="Select Same song for playlist"]')];
    expect(checks).toHaveLength(2);
    checks[1]!.click(); checks[0]!.click();
    await settle();
    target.querySelector<HTMLButtonElement>(".add-selected")!.click();
    expect(onAddSelectedToPlaylist).toHaveBeenCalledWith([first, second]);
  });
  it("keeps release titles inside cards, formats dates, and uses a compact left-aligned artist header", async () => {
    const longTitle = "A Very Long Live Recording Title That Must Stay Inside Its Release Card";
    const datedAlbum = { ...album("long-title", longTitle), releaseDate: "2025-08-13T00:00:00Z" };
    const call = vi.fn(async (command: string) => {
      if (command === "browse_remote_artist") return {
        artist: { id: "artist-id", name: "Radiohead", url: "https://open.spotify.com/artist/artist-id", genres: [] },
        albums: [datedAlbum], topTracksUnavailable: "No top tracks were supplied.",
      };
      if (command === "browse_remote_artist_tracks") return { provider: "spotify", artistId: "artist-id", tracks: [], total: 0 };
      throw new Error(`Unexpected command ${command}`);
    });
    const target = render(call); await settle();
    await vi.waitFor(() => expect(target.querySelector('[role="status"]')?.textContent).toBe("No top tracks were supplied."));
    const title = target.querySelector<HTMLButtonElement>(".album strong")!;
    expect(title.textContent).toBe(longTitle);
    expect(title.title).toBe(longTitle);
    expect(target.querySelector(".album span:last-child")?.textContent).toBe("Aug 13, 2025");
    expect(target.querySelector(".hero .placeholder")).not.toBeNull();
  });

  it("uses artwork from the exact Spotify search result only when the artist page omits it", async () => {
    const artwork = "https://i.scdn.co/image/exact-artist-image";
    const call = vi.fn(async (command: string) => {
      if (command === "browse_remote_artist") return {
        artist: { id: "artist-id", name: "Fixture Artist", url: "https://open.spotify.com/artist/artist-id", genres: [] },
        albums: [], topTracksUnavailable: "Unavailable",
      };
      if (command === "browse_remote_artist_tracks") return { provider: "spotify", artistId: "artist-id", tracks: [], total: 0 };
      throw new Error(`Unexpected command ${command}`);
    });
    const target = render(call, undefined, "artist-id", { searchArtistFallback: { id: "artist-id", title: "Fixture Artist", url: null, artworkUrl: artwork } }); await settle();
    expect(target.querySelector<HTMLImageElement>(".hero img")?.src).toBe(artwork);
    destroy?.(); destroy = undefined;
    const mismatched = render(call, undefined, "artist-id", { searchArtistFallback: { id: "different-artist", title: "Other Artist", url: null, artworkUrl: artwork } }); await settle();
    expect(mismatched.querySelector(".hero img")).toBeNull();
    expect(mismatched.querySelector(".hero .placeholder")).not.toBeNull();
  });

  it("loads paged top tracks, keeps duplicate occurrences through sorting and playback, and restores the loaded snapshot", async () => {
    const id = "top-track-artist";
    const status = { accountRevisions: { spotify: 8 }, spotifyAccountId: "top-track-account" };
    const repeated = { ...track("repeated", "Alpha"), provider: "spotify", providerId: "spotify:repeated", sourceUrl: "https://open.spotify.com/track/repeated" };
    const zulu = { ...track("zulu", "Zulu"), provider: "spotify", providerId: "spotify:zulu", sourceUrl: "https://open.spotify.com/track/zulu" };
    const topPage = (args: Record<string, unknown>) => args.cursor
      ? { provider: "spotify", accountId: "top-track-account", artistId: id, tracks: [repeated], total: 3 }
      : { provider: "spotify", accountId: "top-track-account", artistId: id, tracks: [zulu, repeated], nextCursor: "top-2", total: 3 };
    const call = vi.fn(async (command: string, args: Record<string, unknown>) => {
      if (command === "get_managed_provider_session_status") return status;
      if (command === "browse_remote_artist") return { artist: { id, name: "Top Track Artist", url: `https://open.spotify.com/artist/${id}`, genres: [] }, albums: [] };
      if (command === "browse_remote_artist_tracks") return topPage(args);
      throw new Error(`Unexpected command ${command}`);
    });
    const onPlay = vi.fn();
    const target = render(call, onPlay, id); await settle();
    expect(call).toHaveBeenCalledWith("browse_remote_artist_tracks", expect.objectContaining({ artistId: id, cursor: null, requestId: expect.any(String) }));
    expect(target.querySelectorAll('ol[aria-label="Top Track Artist top tracks"] li')).toHaveLength(2);
    const sort = target.querySelector<HTMLSelectElement>('[aria-label="Sort top tracks"]')!;
    sort.value = "title"; sort.dispatchEvent(new Event("change", { bubbles: true })); await settle();
    [...target.querySelectorAll<HTMLButtonElement>("button")].find(button => button.textContent === "Load more top tracks")!.click(); await settle();
    const list = target.querySelector('ol[aria-label="Top Track Artist top tracks"]')!;
    expect([...list.querySelectorAll(".track-title")].map(button => button.textContent?.trim())).toEqual(["Alpha", "Alpha", "Zulu"]);
    [...list.querySelectorAll<HTMLButtonElement>('button.track-title[aria-label="Play Alpha"]')][1]!.click(); await settle();
    expect(onPlay).toHaveBeenLastCalledWith(repeated, [repeated, repeated, zulu], 1);
    expect(target.textContent).toContain("3 matching · 3 loaded of 3");
    destroy?.(); destroy = undefined;

    const restoredCall = vi.fn(async (command: string) => {
      if (command === "get_managed_provider_session_status") return status;
      throw new Error(`Unexpected command ${command}`);
    });
    const restored = render(restoredCall, undefined, id); await settle();
    expect(restoredCall).not.toHaveBeenCalledWith("browse_remote_artist_tracks", expect.anything());
    expect(restored.querySelector<HTMLSelectElement>('[aria-label="Sort top tracks"]')?.value).toBe("title");
    expect(restored.querySelectorAll('ol[aria-label="Top Track Artist top tracks"] li')).toHaveLength(3);
    expect(restored.textContent).toContain("3 matching · 3 loaded of 3");
  });

  it("keeps sort fields enabled when duration filters hide every loaded track", async () => {
    const id = "availability-after-filter";
    const albumPage = album("availability-album", "Availability Album");
    const recording = { ...track("recording", "Known Track"), provider: "spotify", providerId: "spotify:recording", sourceUrl: "https://open.spotify.com/track/recording" };
    const call = vi.fn(async (command: string, args: Record<string, unknown>) => {
      if (command === "browse_remote_artist") return { artist: { id, name: "Availability Artist", url: `https://open.spotify.com/artist/${id}`, genres: [] }, albums: [albumPage] };
      if (command === "browse_remote_artist_tracks") return { provider: "spotify", artistId: id, tracks: [recording], total: 1 };
      if (command === "browse_remote_album_tracks") return { album: albumPage, tracks: [recording], total: 1 };
      throw new Error(`Unexpected command ${command}`);
    });
    const target = render(call, undefined, id); await settle();
    await vi.waitFor(() => expect(target.querySelector('[aria-label="Minimum top track duration"]')).not.toBeNull());
    const topSort = target.querySelector<HTMLSelectElement>('[aria-label="Sort top tracks"]')!;
    const topMinimum = target.querySelector<HTMLInputElement>('[aria-label="Minimum top track duration"]')!;
    topMinimum.value = "500"; topMinimum.dispatchEvent(new Event("input", { bubbles: true })); await settle();
    expect(target.querySelectorAll('ol[aria-label="Availability Artist top tracks"] li')).toHaveLength(0);
    expect(topSort.querySelector<HTMLOptionElement>('option[value="title"]')?.disabled).toBe(false);
    expect(topSort.querySelector<HTMLOptionElement>('option[value="duration"]')?.disabled).toBe(false);

    target.querySelector<HTMLButtonElement>("button.album")!.click(); await settle();
    await vi.waitFor(() => expect(target.querySelector('[aria-label="Sort tracks"]')).not.toBeNull());
    const albumSort = target.querySelector<HTMLSelectElement>('[aria-label="Sort tracks"]')!;
    const albumMinimum = target.querySelector<HTMLInputElement>('[aria-label="Minimum track duration"]')!;
    albumMinimum.value = "500"; albumMinimum.dispatchEvent(new Event("input", { bubbles: true })); await settle();
    expect(target.querySelectorAll(".shared-track-list li")).toHaveLength(0);
    expect(albumSort.querySelector<HTMLOptionElement>('option[value="title"]')?.disabled).toBe(false);
    expect(albumSort.querySelector<HTMLOptionElement>('option[value="duration"]')?.disabled).toBe(false);
  });

  it("keeps sort fields available until Spotify track pages are exhausted", async () => {
    const id = "paged-sort-fields";
    const albumPage = album("paged-sort-album", "Paged Sort Album");
    const first = { ...track("first", "First"), bpm: undefined };
    const second = { ...track("second", "Second"), bpm: 128 };
    const call = vi.fn(async (command: string, args: Record<string, unknown>) => {
      if (command === "get_managed_provider_session_status") return { accountRevisions: { spotify: 8 }, spotifyAccountId: "paged-sort-account" };
      if (command === "browse_remote_artist") return { artist: { id, name: "Paged Artist", url: `https://open.spotify.com/artist/${id}`, genres: [] }, albums: [albumPage] };
      if (command === "browse_remote_artist_tracks") return args.cursor
        ? { provider: "spotify", artistId: id, tracks: [second], total: 2 }
        : { provider: "spotify", artistId: id, tracks: [first], nextCursor: "top-next", total: 2 };
      if (command === "browse_remote_album_tracks") return args.cursor
        ? { album: albumPage, tracks: [second], total: 2 }
        : { album: albumPage, tracks: [first], nextCursor: "album-next", total: 2 };
      throw new Error(`Unexpected command ${command}`);
    });
    const target = render(call, undefined, id); await settle();
    expect(target.querySelector<HTMLSelectElement>('[aria-label="Sort top tracks"]')?.querySelector<HTMLOptionElement>('option[value="bpm"]')?.disabled).toBe(false);
    [...target.querySelectorAll<HTMLButtonElement>("button")].find(button => button.textContent === "Load more top tracks")!.click(); await settle();
    expect(target.querySelector<HTMLSelectElement>('[aria-label="Sort top tracks"]')?.querySelector<HTMLOptionElement>('option[value="bpm"]')?.disabled).toBe(false);
    target.querySelector<HTMLButtonElement>("button.album")!.click(); await settle();
    expect(target.querySelector<HTMLSelectElement>('[aria-label="Sort tracks"]')?.querySelector<HTMLOptionElement>('option[value="bpm"]')?.disabled).toBe(false);
    [...target.querySelectorAll<HTMLButtonElement>("button")].find(button => button.textContent === "Load more tracks")!.click(); await settle();
    expect(target.querySelector<HTMLSelectElement>('[aria-label="Sort tracks"]')?.querySelector<HTMLOptionElement>('option[value="bpm"]')?.disabled).toBe(false);
  });

  it("does not cancel a release page when loading the next top-tracks page", async () => {
    const id = "independent-artist-reads";
    const secondReleases = deferred<unknown>();
    const releaseAlbum = album("independent-album", "Independent album");
    const call = vi.fn(async (command: string, args: Record<string, unknown> = {}) => {
      if (command === "browse_remote_artist") return args.cursor
        ? secondReleases.promise
        : { artist: { id, name: "Independent Artist", url: `https://open.spotify.com/artist/${id}`, genres: [] }, albums: [releaseAlbum], nextCursor: "release-2", totalAlbums: 2 };
      if (command === "browse_remote_artist_tracks") return args.cursor
        ? { provider: "spotify", artistId: id, tracks: [track("top-2", "Top two")], total: 2 }
        : { provider: "spotify", artistId: id, tracks: [track("top-1", "Top one")], nextCursor: "top-2", total: 2 };
      if (command === "cancel_remote_read") return null;
      throw new Error(`Unexpected command ${command}`);
    });
    const target = render(call, undefined, id); await settle();
    await vi.waitFor(() => expect([...target.querySelectorAll<HTMLButtonElement>("button")].some(button => button.textContent === "Load more top tracks")).toBe(true));
    [...target.querySelectorAll<HTMLButtonElement>("button")].find(button => button.textContent?.includes("Load more releases"))!.click();
    await vi.waitFor(() => expect(call.mock.calls.some(([command, args]) => command === "browse_remote_artist" && args?.cursor === "release-2")).toBe(true));
    const releaseRequestId = call.mock.calls.find(([command, args]) => command === "browse_remote_artist" && args?.cursor === "release-2")?.[1]?.requestId;
    [...target.querySelectorAll<HTMLButtonElement>("button")].find(button => button.textContent === "Load more top tracks")!.click();
    await vi.waitFor(() => expect(target.querySelectorAll('ol[aria-label="Independent Artist top tracks"] li')).toHaveLength(2));
    expect(call.mock.calls.some(([command, args]) => command === "cancel_remote_read" && args?.requestId === releaseRequestId)).toBe(false);
    secondReleases.resolve({ artist: { id, name: "Independent Artist", url: `https://open.spotify.com/artist/${id}`, genres: [] }, albums: [album("independent-album-2", "Second album")], totalAlbums: 2 });
    await settle();
    expect(target.querySelectorAll("button.album")).toHaveLength(2);
    expect(target.querySelectorAll('ol[aria-label="Independent Artist top tracks"] li')).toHaveLength(2);
  });

  it("ignores a cancelled initial top-tracks response and retries when returning from an album", async () => {
    const id = "cancelled-top-read";
    const late = deferred<unknown>();
    const albumPage = album("cancelled-album", "Cancelled album");
    let topCall = 0;
    const call = vi.fn(async (command: string, args: Record<string, unknown> = {}) => {
      if (command === "browse_remote_artist") return { artist: { id, name: "Cancellation Artist", url: `https://open.spotify.com/artist/${id}`, genres: [] }, albums: [albumPage] };
      if (command === "browse_remote_artist_tracks") {
        topCall += 1;
        return topCall === 1 ? late.promise : { provider: "spotify", artistId: id, tracks: [track("fresh", "Fresh track")], total: 1 };
      }
      if (command === "browse_remote_album_tracks") return { album: albumPage, tracks: [track("album-track", "Album track")], total: 1 };
      if (command === "cancel_remote_read") return null;
      throw new Error(`Unexpected command ${command}`);
    });
    const target = render(call, undefined, id); await settle();
    await vi.waitFor(() => expect(call.mock.calls.some(([command]) => command === "browse_remote_artist_tracks")).toBe(true));
    const staleRequestId = call.mock.calls.find(([command]) => command === "browse_remote_artist_tracks")?.[1]?.requestId;
    target.querySelector<HTMLButtonElement>("button.album")!.click(); await settle();
    expect(call.mock.calls.some(([command, args]) => command === "cancel_remote_read" && args?.requestId === staleRequestId)).toBe(true);
    [...target.querySelectorAll<HTMLButtonElement>("button")].find(button => button.textContent === "Back to releases")!.click(); await settle();
    await vi.waitFor(() => expect(topCall).toBe(2));
    late.resolve({ provider: "spotify", artistId: id, tracks: [track("stale", "Stale track")], total: 1 });
    await settle();
    expect(target.textContent).not.toContain("Stale track");
    await vi.waitFor(() => expect(target.querySelectorAll('ol[aria-label="Cancellation Artist top tracks"] li')).toHaveLength(1));
    expect(target.textContent).toContain("Fresh track");
  });

  it("sorts all loaded releases with stable missing-last dates and restores release controls", async () => {
    const id = "release-sort-artist";
    const status = { accountRevisions: { spotify: 4 }, spotifyAccountId: "release-sort-account" };
    const newest = { ...album("new", "New release"), releaseDate: "2024-05-01" };
    const undated = album("unknown", "Undated release");
    const oldest = { ...album("old", "Old release"), releaseDate: "2001" };
    const sameDate = { ...album("same", "Same date release"), releaseDate: "2024-05-01" };
    const call = vi.fn(async (command: string, args: Record<string, unknown>) => {
      if (command === "get_managed_provider_session_status") return status;
      if (command === "browse_remote_artist") return args.cursor
        ? { artist: { id, name: "Release Sort Artist", url: `https://open.spotify.com/artist/${id}`, genres: [] }, albums: [oldest, sameDate], totalAlbums: 4 }
        : { artist: { id, name: "Release Sort Artist", url: `https://open.spotify.com/artist/${id}`, genres: [] }, albums: [newest, undated], nextCursor: "releases-2", totalAlbums: 4 };
      throw new Error(`Unexpected command ${command}`);
    });
    const target = render(call, undefined, id); await settle();
    [...target.querySelectorAll<HTMLButtonElement>("button")].find(button => button.textContent?.includes("Load more releases"))!.click(); await settle();
    expect(target.querySelectorAll("button.album")).toHaveLength(4);
    const titles = () => [...target.querySelectorAll<HTMLButtonElement>("button.album strong")].map(node => node.textContent);
    const field = target.querySelector<HTMLSelectElement>('[aria-label="Sort releases"]')!;
    const direction = target.querySelector<HTMLSelectElement>('[aria-label="Release sort direction"]')!;
    const change = (select: HTMLSelectElement, value: string) => { select.value = value; select.dispatchEvent(new Event("change", { bubbles: true })); };
    change(field, "releaseDate"); change(direction, "asc"); await settle();
    expect(titles()).toEqual(["Old release", "New release", "Same date release", "Undated release"]);
    change(direction, "desc"); await settle();
    expect(titles()).toEqual(["New release", "Same date release", "Old release", "Undated release"]);

    destroy?.(); destroy = undefined;
    const restoredCall = vi.fn(async (command: string) => {
      if (command === "get_managed_provider_session_status") return status;
      throw new Error(`Unexpected command ${command}`);
    });
    const restored = render(restoredCall, undefined, id); await settle();
    expect(restoredCall).not.toHaveBeenCalledWith("browse_remote_artist", expect.anything());
    expect(restored.querySelector<HTMLSelectElement>('[aria-label="Sort releases"]')?.value).toBe("releaseDate");
    expect(restored.querySelector<HTMLSelectElement>('[aria-label="Release sort direction"]')?.value).toBe("desc");
    expect([...restored.querySelectorAll<HTMLButtonElement>("button.album strong")].map(node => node.textContent)).toEqual(["New release", "Same date release", "Old release", "Undated release"]);
  });

  it("restores the artist album, loaded track pages, sort controls, and scroll for the same account revision", async () => {
    const id = "navigation-artist";
    const status = { accountRevisions: { spotify: 12 }, spotifyAccountId: "fixture-account" };
    const call = vi.fn(async (command: string, args: Record<string, unknown>) => {
      if (command === "get_managed_provider_session_status") return status;
      if (command === "browse_remote_artist") return { artist: { id, name: "Fixture Artist", url: `https://open.spotify.com/artist/${id}`, genres: [] }, albums: [album("snapshot-album", "Snapshot album")], topTracksUnavailable: "Unavailable" };
      if (command === "browse_remote_album_tracks") return args.cursor
        ? { album: album("snapshot-album", "Snapshot album"), tracks: [track("snapshot-track-2", "Second track")], total: 2 }
        : { album: album("snapshot-album", "Snapshot album"), tracks: [track("snapshot-track-1", "First track")], nextCursor: "tracks-2", total: 2 };
      throw new Error(`Unexpected ${command}`);
    });
    const target = render(call, undefined, id); await settle();
    target.querySelector<HTMLButtonElement>("button.album")!.click(); await settle();
    [...target.querySelectorAll<HTMLButtonElement>("button")].find(button => button.textContent?.includes("Load more tracks"))!.click(); await settle();
    const sort = target.querySelector<HTMLSelectElement>('[aria-label="Sort tracks"]')!;
    sort.value = "title"; sort.dispatchEvent(new Event("change")); await settle();
    const page = target.querySelector<HTMLElement>(".spotify-artist")!;
    page.scrollTop = 41; page.dispatchEvent(new Event("scroll"));
    destroy?.(); destroy = undefined;

    const restoredCall = vi.fn(async (command: string) => {
      if (command === "get_managed_provider_session_status") return status;
      throw new Error(`Unexpected ${command}`);
    });
    const restored = render(restoredCall, undefined, id); await settle();
    expect(restoredCall).not.toHaveBeenCalledWith("browse_remote_artist", expect.anything());
    expect(restoredCall).not.toHaveBeenCalledWith("browse_remote_album_tracks", expect.anything());
    expect(restored.textContent).toContain("Snapshot album");
    expect(restored.querySelectorAll(".shared-track-list > li:not(.virtual-spacer)")).toHaveLength(2);
    expect(restored.querySelector<HTMLSelectElement>('[aria-label="Sort tracks"]')?.value).toBe("title");
    expect(restored.querySelector<HTMLElement>(".spotify-artist")?.scrollTop).toBe(41);
  });

  it("deduplicates repeated releases, preserves recording occurrences, and stops repeated cursors", async () => {
    const call = vi.fn(async (command: string, args: Record<string, unknown>) => {
      if (command === "browse_remote_artist") return args.cursor
        ? { artist: { id: "artist-id", name: "Fixture Artist", url: "https://open.spotify.com/artist/artist-id", genres: [] }, albums: [album("a2", "Second duplicate"), album("a3", "Third")], nextCursor: "releases-2", topTracksUnavailable: "Unavailable" }
        : { artist: { id: "artist-id", name: "Fixture Artist", url: "https://open.spotify.com/artist/artist-id", genres: [] }, albums: [album("a1", "First"), album("a2", "Second")], nextCursor: "releases-2", topTracksUnavailable: "Unavailable" };
      if (command === "browse_remote_album_tracks") return args.cursor
        ? { album: album("a1", "First"), tracks: [track("t1", "One duplicate"), track("t2", "Two"), track("t2", "Two duplicate")], nextCursor: "tracks-2" }
        : { album: album("a1", "First"), tracks: [track("t1", "One")], nextCursor: "tracks-2" };
      throw new Error(`Unexpected command ${command}`);
    });
    const target = render(call); await settle();
    [...target.querySelectorAll<HTMLButtonElement>("button")].find(button => button.textContent?.includes("Load more releases"))!.click(); await settle();
    expect(target.querySelectorAll("button.album")).toHaveLength(3);
    expect(target.textContent).toContain("repeated a releases page");
    target.querySelector<HTMLButtonElement>("button.album")!.click(); await settle();
    expect(target.querySelectorAll(".shared-track-list > li:not(.virtual-spacer)")).toHaveLength(1);
    [...target.querySelectorAll<HTMLButtonElement>("button")].find(button => button.textContent?.includes("Load more tracks"))!.click(); await settle();
    // Opaque legacy cursors provide no safe overlap identity, so repeated recordings remain.
    expect(target.querySelectorAll(".shared-track-list > li:not(.virtual-spacer)")).toHaveLength(4);
    expect(target.textContent).toContain("One duplicate");
    expect(target.textContent).toContain("Two duplicate");
    expect(target.querySelector("button")?.textContent).toContain("Back");
  });
  it("deduplicates overlapping album occurrences and plays the selected index after sorting", async () => {
    const occurrence = (title: string, position: number) => ({
      ...track("same-recording", title),
      provenance: { remoteEntryId: "album:a1:" + (position - 1), position, positionScope: "album" },
    });
    const played = vi.fn();
    const call = vi.fn(async (command: string, args: Record<string, unknown>) => {
      if (command === "browse_remote_artist") return { artist: { id: "artist-id", name: "Fixture Artist", url: "https://open.spotify.com/artist/artist-id", genres: [] }, albums: [album("a1", "First")], topTracksUnavailable: "Unavailable" };
      if (command === "browse_remote_album_tracks") return args.cursor
        ? { album: album("a1", "First"), tracks: [occurrence("Alpha", 2), occurrence("Aardvark", 4)], total: 4 }
        : { album: album("a1", "First"), tracks: [occurrence("Zulu", 1), occurrence("Alpha", 2), occurrence("Beta", 3)], nextCursor: "3", total: 4 };
      throw new Error("Unexpected command " + command);
    });
    const target = render(call, played); await settle();
    target.querySelector<HTMLButtonElement>("button.album")!.click(); await settle();
    [...target.querySelectorAll<HTMLButtonElement>("button")].find(button => button.textContent?.includes("Load more tracks"))!.click(); await settle();
    const rows = () => [...target.querySelectorAll(".shared-track-list > li:not(.virtual-spacer)")];
    expect(rows()).toHaveLength(4);
    expect(rows().map(row => row.querySelector(".track-title")?.textContent)).toEqual(["Zulu", "Alpha", "Beta", "Aardvark"]);
    const sort = target.querySelector<HTMLSelectElement>('select[aria-label="Sort tracks"]')!;
    sort.value = "title"; sort.dispatchEvent(new Event("change")); await settle();
    expect(rows().map(row => row.querySelector(".track-title")?.textContent)).toEqual(["Aardvark", "Alpha", "Beta", "Zulu"]);
    rows()[1].querySelector<HTMLButtonElement>(".track-title")!.click();
    expect(played).toHaveBeenCalledWith(expect.objectContaining({ title: "Alpha" }), expect.any(Array), 1);
  });
  it("stops a multi-page album cursor cycle without discarding collected tracks", async () => {
    const call = vi.fn(async (command: string, args: Record<string, unknown>) => {
      if (command === "browse_remote_artist") return { artist: { id: "artist-id", name: "Fixture Artist", url: "https://open.spotify.com/artist/artist-id", genres: [] }, albums: [album("a1", "First")], topTracksUnavailable: "Unavailable" };
      if (command === "browse_remote_album_tracks") {
        if (!args.cursor) return { album: album("a1", "First"), tracks: [track("t1", "First")], nextCursor: "cursor-a", total: 3 };
        if (args.cursor === "cursor-a") return { album: album("a1", "First"), tracks: [track("t2", "Second")], nextCursor: "cursor-b", total: 3 };
        return { album: album("a1", "First"), tracks: [track("t1", "Repeated first")], nextCursor: "cursor-a", total: 3 };
      }
      throw new Error("Unexpected command " + command);
    });
    const target = render(call); await settle();
    target.querySelector<HTMLButtonElement>("button.album")!.click(); await settle();
    const more = () => [...target.querySelectorAll<HTMLButtonElement>("button")].find(button => button.textContent?.includes("Load more tracks"));
    more()!.click(); await settle();
    more()!.click(); await settle();
    expect(more()).toBeUndefined();
    expect(target.querySelectorAll(".shared-track-list > li:not(.virtual-spacer)")).toHaveLength(3);
    expect(target.textContent).toContain("repeated a tracks page");
    expect(call.mock.calls.filter(([command]) => command === "browse_remote_album_tracks")).toHaveLength(3);
  });
  it("sorts all collected album pages with shared fields and whole-second duration bounds", async () => {
    const call = vi.fn(async (command: string, args: Record<string, unknown>) => {
      if (command === "browse_remote_artist") return { artist: { id: "artist-id", name: "Fixture Artist", url: "https://open.spotify.com/artist/artist-id", genres: [] }, albums: [album("a1", "First")], topTracksUnavailable: "Unavailable" };
      if (command === "browse_remote_album_tracks") return args.cursor
        ? { album: album("a1", "First"), tracks: [{ ...track("t3", "Fast"), bpm: 140, durationSeconds: 180.9 }], total: 3 }
        : { album: album("a1", "First"), tracks: [{ ...track("t1", "Unknown"), durationSeconds: 181 }, { ...track("t2", "Slow"), bpm: 80, durationSeconds: 180.1 }], nextCursor: "page-2", total: 3 };
      throw new Error(`Unexpected command ${command}`);
    });
    const target = render(call); await settle();
    target.querySelector<HTMLButtonElement>("button.album")!.click(); await settle();
    const sort = target.querySelector<HTMLSelectElement>('select[aria-label="Sort tracks"]')!;
    expect([...sort.options].map(option => option.value)).toEqual(["sourceOrder", "title", "artist", "duration", "album", "releaseDate", "dateAdded", "bpm", "key", "plays", "views"]);
    sort.value = "bpm"; sort.dispatchEvent(new Event("change"));
    const direction = target.querySelector<HTMLSelectElement>('select[aria-label="Sort direction"]')!;
    direction.value = "desc"; direction.dispatchEvent(new Event("change")); await settle();
    [...target.querySelectorAll<HTMLButtonElement>("button")].find(button => button.textContent?.includes("Load more tracks"))!.click(); await settle();
    const rows = () => [...target.querySelectorAll(".shared-track-list > li:not(.virtual-spacer)")].map(row => row.textContent);
    expect(rows()[0]).toContain("Fast"); expect(rows()[1]).toContain("Slow"); expect(rows()[2]).toContain("Unknown");
    expect(target.textContent).toContain("collected tracks (3 of 3)");
    for (const label of ["Minimum track duration", "Maximum track duration"]) {
      const input = target.querySelector<HTMLInputElement>(`input[aria-label="${label}"]`)!;
      input.value = "3:00"; input.dispatchEvent(new Event("input"));
    }
    await settle(); expect(rows()).toHaveLength(2); expect(rows().join(" ")).not.toContain("Unknown");
  });

});

describe("SpotifyArtist playback row actions", () => {
  it("uses the shared TrackList play, queue, favorite, download, artist, and context actions for album tracks", async () => {
    const id = "playback-action-artist";
    const release = album("playback-actions-album", "Playback actions");
    const song = track("playback-actions-track", "Playback action song");
    const call = vi.fn(async (command: string) => {
      if (command === "browse_remote_artist") return { artist: { id, name: "Playback Artist", url: `https://open.spotify.com/artist/${id}`, genres: [] }, albums: [release], topTracksUnavailable: "Unavailable" };
      if (command === "browse_remote_artist_tracks") return { tracks: [], total: 0 };
      if (command === "browse_remote_album_tracks") return { album: release, tracks: [song], total: 1, complete: true };
      throw new Error(`Unexpected command ${command}`);
    });
    const onPlay = vi.fn(), onPlaylist = vi.fn(), onNext = vi.fn(), onQueue = vi.fn();
    const onFavorite = vi.fn(), onDownload = vi.fn(), onArtist = vi.fn(), onContext = vi.fn();
    const target = render(call, onPlay, id, { onPlaylist, onNext, onQueue, onFavorite, onDownload, onArtist, onContext });
    await settle();
    target.querySelector<HTMLButtonElement>("button.album")!.click();
    await settle();

    const row = target.querySelector<HTMLLIElement>(".shared-track-list li")!;
    row.querySelector<HTMLButtonElement>('.track-title[aria-label="Play Playback action song"]')!.click();
    row.querySelector<HTMLButtonElement>('.icon-action.favorite[aria-label="Add Playback action song to favorites"]')!.click();
    row.querySelector<HTMLButtonElement>('.icon-action[aria-label="Download Playback action song"]')!.click();
    row.querySelector<HTMLButtonElement>('.more summary')!.click();
    const actions = [...row.querySelectorAll<HTMLButtonElement>(".more-menu button")];
    actions.find(button => button.textContent?.includes("Play next"))!.click();
    row.querySelector<HTMLButtonElement>('.more summary')!.click();
    [...row.querySelectorAll<HTMLButtonElement>(".more-menu button")].find(button => button.textContent?.includes("Add to queue"))!.click();
    row.querySelector<HTMLButtonElement>('.more summary')!.click();
    [...row.querySelectorAll<HTMLButtonElement>(".more-menu button")].find(button => button.textContent?.includes("Add to playlist"))!.click();
    row.querySelector<HTMLButtonElement>('.more summary')!.click();
    [...row.querySelectorAll<HTMLButtonElement>(".more-menu button")].find(button => button.textContent?.includes("Open artist"))!.click();
    row.dispatchEvent(new MouseEvent("contextmenu", { bubbles: true, cancelable: true }));

    expect(onPlay).toHaveBeenCalledWith(song, [song], 0);
    expect(onFavorite).toHaveBeenCalledWith(song);
    expect(onDownload).toHaveBeenCalledWith(song);
    expect(onNext).toHaveBeenCalledWith(song);
    expect(onQueue).toHaveBeenCalledWith(song);
    expect(onPlaylist).toHaveBeenCalledWith(song);
    expect(onArtist).toHaveBeenCalledWith(song);
    expect(onContext).toHaveBeenCalledWith(song, expect.any(MouseEvent));
  });
});

describe("per-entry artist navigation snapshots",()=>{
  it("restores distinct controls for two history entries of the same artist",async()=>{
    // @ts-expect-error Exercise the same browser runtime as the component.
    const {writable,fromStore}=await import("../node_modules/svelte/src/store/index-client.js");
    const store=writable(null as string|null),model=fromStore(store);
    const call=vi.fn(async(command:string)=>{
      if(command==="get_managed_provider_session_status")return {spotifyAccountId:"fixture",accountRevisions:{spotify:1}};
      if(command==="browse_remote_artist")return {artist:{id:"artist-id",name:"Fixture",url:"https://open.spotify.com/artist/artist-id",genres:[]},albums:[album("a","A")],topTracksUnavailable:"None"};
      if(command==="browse_remote_artist_tracks")return {tracks:[],nextCursor:null,total:0};
      if(command==="cancel_remote_read")return undefined;
      throw Error(`Unexpected ${command}`);
    });
    const target=document.createElement("div");document.body.append(target);
    const instance=mount(SpotifyArtist,{target,props:{artistId:"artist-id",call,get navigationSnapshotKey(){return model.current}}});destroy=()=>unmount(instance);await settle();await settle();
    const select=target.querySelector<HTMLSelectElement>('[aria-label="Sort releases"]')!;
    select.value="title";select.dispatchEvent(new Event("change",{bubbles:true}));await settle();
    instance.captureNavigationSnapshot("history-a");
    select.value="releaseDate";select.dispatchEvent(new Event("change",{bubbles:true}));await settle();
    instance.captureNavigationSnapshot("history-b");
    store.set("history-a");await settle();await settle();
    expect(target.querySelector<HTMLSelectElement>('[aria-label="Sort releases"]')?.value).toBe("title");
    store.set("history-b");await settle();await settle();
    expect(target.querySelector<HTMLSelectElement>('[aria-label="Sort releases"]')?.value).toBe("releaseDate");
    store.set("history-a");await settle();await settle();
    expect(target.querySelector<HTMLSelectElement>('[aria-label="Sort releases"]')?.value).toBe("title");
  });
});
