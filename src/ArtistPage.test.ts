// @vitest-environment jsdom
// @ts-expect-error Exercise Svelte's browser runtime in jsdom.
import { mount, tick, unmount } from "../node_modules/svelte/src/index-client.js";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { ArtistProfile, ArtistReleasePage, ArtistReleaseRef, ArtistSearchResult } from "./lib/artist-types";
import type { Track } from "./lib/types";

vi.mock("svelte", async () => {
  // @ts-expect-error Match the browser runtime used by the direct import above.
  return await import("../node_modules/svelte/src/index-client.js");
});
import ArtistPage from "./ArtistPage.svelte";
import { providerNavigationSnapshots } from "./lib/provider-navigation-snapshots";

const artist = (id: string, name = id): ArtistSearchResult => ({
  artist: { provider: "musicbrainz", id }, name, sourceUrl: `https://musicbrainz.org/artist/${id}`,
});
const release = (id: string, title: string, sourceUrl: string, primaryType: string): ArtistReleaseRef => ({
  release: { provider: "musicbrainz", id }, title, sourceUrl, primaryType, tracks: [],
  credits: [{ name: "Big Thief", role: "artist", sourceUrl }],
});
const emptyReleasePage = (artistId: string, items: ArtistReleaseRef[] = [], nextCursor: string | null = null): ArtistReleasePage => ({
  artist: { provider: "musicbrainz", id: artistId }, items, nextCursor, sourceCoverage: [], warnings: [],
});
function profile(result: ArtistSearchResult, page = emptyReleasePage(result.artist.id)): ArtistProfile {
  return {
    artist: result, aliases: [], biography: null, biographySource: null, imageUrl: null,
    genres: [], tags: [], sourceTags: [], links: [], topTracks: [], releases: page,
    discogsAvailable: false, lastfmAvailable: false, sourceCoverage: [], warnings: [],
  };
}

let destroy: (() => void) | undefined;
let snapshotInstance: {captureNavigationSnapshot:(key:string)=>string;restoreNavigationSnapshot:(key:string)=>Promise<void>};
afterEach(() => { destroy?.(); destroy = undefined; document.body.innerHTML = ""; vi.restoreAllMocks(); });
async function settle() { for (let i = 0; i < 10; i++) await Promise.resolve(); await tick(); }
function render(props: Record<string, unknown>) {
  const target = document.createElement("div"); document.body.append(target);
  const instance = mount(ArtistPage, { target, props: {
    activeTrack: null, localTracks: [], searchArtists: vi.fn(async () => ({ query: "", page: 1, pageSize: 25, hasMore: false, results: [], warnings: [] })),
    getArtistProfile: vi.fn(async (ref: { id: string }) => profile(artist(ref.id))),
    getArtistReleases: vi.fn(async (ref: { id: string }) => emptyReleasePage(ref.id)),
    getArtistTracks: vi.fn(async (ref: { id: string }) => ({ artist: ref, items: [], source: "MusicBrainz", warnings: [] })),
    getArtistCollaborations: vi.fn(async (ref: { id: string }) => ({ artist: ref, items: [], sourceCoverage: [], warnings: [] })),
    resolveRecordingMetadata: vi.fn(), onApplyMetadata: vi.fn(), ...props,
  } as never });
  snapshotInstance=instance;
  destroy = () => unmount(instance);
  return target;
}

describe("ArtistPage", () => {
  it("renders albums with repeated unnumbered songs and repeated disc positions", async () => {
    const gillian = artist("gillian-welch", "Gillian Welch");
    const live = release("live", "The Palms, Davis CA", "https://musicbrainz.org/release/live", "Album");
    live.tracks = [
      { position: null, title: "Caleb Meyer", sourceUrl: live.sourceUrl },
      { position: null, title: "Caleb Meyer", sourceUrl: live.sourceUrl },
      { position: "1", title: "Time (The Revelator)", sourceUrl: live.sourceUrl },
      { position: "1", title: "Time (The Revelator)", sourceUrl: live.sourceUrl },
    ];
    const target = render({ initialArtistRef: gillian.artist, initialSection: "releases",
      getArtistProfile: async () => profile(gillian, emptyReleasePage(gillian.artist.id, [live])),
    });
    await settle();
    expect(target.querySelector(".artist-heading h2")?.textContent).toBe("Gillian Welch");
    expect(target.querySelector(".release-list")?.textContent).toContain("The Palms, Davis CA");
    const titles = [...target.querySelectorAll(".release-tracklist li")].map(item => item.textContent);
    expect(titles.filter(title => title?.includes("Caleb Meyer"))).toHaveLength(2);
    expect(titles.filter(title => title?.includes("Time (The Revelator)"))).toHaveLength(2);
  });
  it("offers explicit Spotify discovery for the exact Discogs artist name without changing the active identity", async () => {
    const name = "The Artist — Live & Unfiltered";
    const discogsArtist = artist("discogs-42", name);
    discogsArtist.artist.provider = "discogs";
    discogsArtist.sourceUrl = "https://www.discogs.com/artist/42";
    const onBrowseArtistProvider = vi.fn();
    const target = render({ initialArtistRef: discogsArtist.artist, onBrowseArtistProvider,
      getArtistProfile: async () => profile(discogsArtist),
    });
    await settle();
    expect(onBrowseArtistProvider).not.toHaveBeenCalled();
    const browse = target.querySelector<HTMLButtonElement>(`[aria-label="Find ${name} on Spotify"]`)!;
    expect(browse.textContent).toBe("Find on Spotify");
    browse.click();
    expect(onBrowseArtistProvider).toHaveBeenCalledOnce();
    expect(onBrowseArtistProvider).toHaveBeenCalledWith(name, "spotify");
    expect(target.querySelector(".artist-heading h2")?.textContent).toBe(name);
    expect(target.querySelector(".artist-heading .eyebrow")?.textContent).toContain("Discogs");
  });
  it("finds Gillian Welch on page one, opens her albums, and loads the next artist page in order", async () => {
    const gillian = artist("gillian-welch", "Gillian Welch");
    const other = artist("gillian-philip", "Gillian Philip");
    const searchArtists = vi.fn(async (_query: string, page: number) => ({
      query: "Gillian Welch", page, pageSize: 20, hasMore: page === 1,
      results: page === 1 ? [gillian] : [other], warnings: [],
    }));
    const target = render({ artistName: "Gillian Welch", initialSection: "releases", searchArtists,
      getArtistProfile: async () => profile(gillian, emptyReleasePage(gillian.artist.id, [
        release("revelator", "Time (The Revelator)", "https://musicbrainz.org/release/revelator", "Album"),
      ])),
    });
    await settle();
    expect(searchArtists).toHaveBeenCalledWith("Gillian Welch", 1);
    [...target.querySelectorAll<HTMLButtonElement>("button")].find(b => b.textContent === "Load more artists")!.click();
    await settle();
    expect(searchArtists).toHaveBeenLastCalledWith("Gillian Welch", 2);
    target.querySelector<HTMLButtonElement>(".artist-result")!.click();
    await settle();
    expect(target.querySelector(".artist-heading h2")?.textContent).toBe("Gillian Welch");
    expect(target.querySelector(".tabs button.active")?.textContent).toBe("Discography");
    expect(target.textContent).toContain("Time (The Revelator)");
  });
  it("sorts and duration-filters the loaded artist top tracks", async () => {
    const ref = artist("top-sort", "Top Sort Artist");
    const topTracks = [
      { id: "long", provider: "lastfm", providerId: "long", sourceUrl: "", artist: "Top Sort Artist", title: "Alpha", durationSeconds: 120, favorite: false, provenance: {} },
      { id: "short", provider: "lastfm", providerId: "short", sourceUrl: "", artist: "Top Sort Artist", title: "Beta", durationSeconds: 30, favorite: false, provenance: {} },
      { id: "mid", provider: "lastfm", providerId: "mid", sourceUrl: "", artist: "Top Sort Artist", title: "Gamma", durationSeconds: 60, favorite: false, provenance: {} },
    ];
    const result = profile(ref);
    result.topTracks = topTracks;
    const target = render({ initialArtistRef: ref.artist, initialSection: "overview", getArtistProfile: async () => result });
    await settle();
    const sort = target.querySelector<HTMLSelectElement>('[aria-label="Sort artist top tracks"]')!;
    sort.value = "duration"; sort.dispatchEvent(new Event("change", { bubbles: true }));
    const minimum = target.querySelector<HTMLInputElement>('[aria-label="Minimum artist top track duration"]')!;
    minimum.value = "60"; minimum.dispatchEvent(new Event("input", { bubbles: true }));
    await settle();
    expect([...target.querySelectorAll<HTMLButtonElement>('[aria-label="Artist top tracks"] .track-title')].map(button => button.textContent?.trim())).toEqual(["Gamma", "Alpha"]);
    expect(target.querySelector('[aria-label="Artist top tracks controls"]')?.textContent).toContain("2 of 3 loaded tracks");
  });

  it("loads artist local recordings in explicit 200-row backend pages with global sort/filter arguments", async () => {
    const ref = artist("local-page", "Exact Local Artist");
    const result = profile(ref);
    result.aliases = ["Exact Local Alias"];
    const first = { id: "local-1", provider: "local", providerId: "local-1", sourceUrl: "", artist: "Exact Local Artist", title: "First", durationSeconds: 90, favorite: false, localAvailability: "available", provenance: {} };
    const second = { id: "local-2", provider: "local", providerId: "local-2", sourceUrl: "", artist: "Exact Local Alias", title: "Second", durationSeconds: 120, favorite: false, localAvailability: "available", provenance: {} };
    const loadPage = vi.fn(async (request: { offset: number; query: string }) => ({
      items: request.offset === 0 ? [first] : [second], total: 2, limit: 200, offset: request.offset,
      sortAvailability: { sourceOrder: true, title: true, artist: true, duration: true },
    }));
    const target = render({ initialArtistRef: ref.artist, initialSection: "local", getArtistProfile: async () => result, onLoadLocalAssetsPage: loadPage });
    await settle();
    expect(loadPage).toHaveBeenCalledTimes(1);
    expect(loadPage).toHaveBeenLastCalledWith(expect.objectContaining({ names: ["Exact Local Artist", "Exact Local Alias"], limit: 200, offset: 0, query: "", sortField: "sourceOrder", direction: "asc", availability: "all" }));
    expect(target.querySelectorAll('[aria-label="Local artist recordings"] .track-title')).toHaveLength(1);
    [...target.querySelectorAll<HTMLButtonElement>("button")].find(button => button.textContent === "Load more local recordings")!.click();
    await settle();
    expect(loadPage).toHaveBeenCalledTimes(2);
    expect(loadPage).toHaveBeenLastCalledWith(expect.objectContaining({ offset: 1 }));
    expect(target.querySelectorAll('[aria-label="Local artist recordings"] .track-title')).toHaveLength(2);
    const minimum = target.querySelector<HTMLInputElement>('[aria-label="Minimum local artist duration"]')!;
    minimum.value = "100"; minimum.dispatchEvent(new Event("input", { bubbles: true }));
    await settle();
    expect(loadPage).toHaveBeenLastCalledWith(expect.objectContaining({ offset: 0, minDurationSeconds: 100 }));
  });

  it("restarts a pending Local page with profile aliases and ignores its earlier response", async () => {
    const ref = artist("local-profile-race", "Exact Local Artist");
    let finishProfile!: (value: ArtistProfile) => void;
    const pending: Array<{
      request: { names: string[]; offset: number };
      resolve: (page: { items: typeof fresh[]; total: number; limit: number; offset: number }) => void;
    }> = [];
    const old = { id: "old-alias-local", provider: "local", providerId: "old-alias-local", sourceUrl: "", artist: "Exact Local Artist", title: "Old", favorite: false, localAvailability: "available", provenance: {} };
    const fresh = { id: "fresh-alias-local", provider: "local", providerId: "fresh-alias-local", sourceUrl: "", artist: "Exact Local Alias", title: "Fresh", favorite: false, localAvailability: "available", provenance: {} };
    const loadPage = vi.fn((request: { names: string[]; offset: number }) => new Promise<{ items: typeof fresh[]; total: number; limit: number; offset: number }>(resolve => pending.push({ request, resolve })));
    const target = render({
      initialArtistRef: ref.artist,
      artistName: "Exact Local Artist",
      initialSection: "overview",
      getArtistProfile: () => new Promise<ArtistProfile>(resolve => { finishProfile = resolve; }),
      onLoadLocalAssetsPage: loadPage,
    });
    await settle();

    target.querySelector<HTMLButtonElement>('[aria-label="Artist sections"] button:nth-child(5)')!.click();
    await settle();
    expect(loadPage).toHaveBeenCalledTimes(1);
    expect(pending[0].request).toEqual(expect.objectContaining({ names: ["Exact Local Artist"], offset: 0 }));

    const result = profile(ref);
    result.aliases = ["Exact Local Alias"];
    finishProfile(result);
    await settle();
    expect(loadPage).toHaveBeenCalledTimes(2);
    expect(pending[1].request).toEqual(expect.objectContaining({ names: ["Exact Local Artist", "Exact Local Alias"], offset: 0 }));

    pending[1].resolve({ items: [fresh], total: 1, limit: 200, offset: 0 });
    await settle();
    pending[0].resolve({ items: [old], total: 1, limit: 200, offset: 0 });
    await settle();
    expect(target.querySelector('[aria-label="Local artist recordings"]')?.textContent).toContain("Fresh");
    expect(target.querySelector('[aria-label="Local artist recordings"]')?.textContent).not.toContain("Old");
  });

  it("ignores a stale artist-local page after its filter query changes", async () => {
    const ref = artist("local-stale", "Stale Local Artist");
    const pending: Array<{ request: { query: string }; resolve: (page: { items: typeof fresh[]; total: number; limit: number; offset: number }) => void }> = [];
    const old = { id: "old-local", provider: "local", providerId: "old-local", sourceUrl: "", artist: "Stale Local Artist", title: "Old", favorite: false, localAvailability: "available", provenance: {} };
    const fresh = { id: "fresh-local", provider: "local", providerId: "fresh-local", sourceUrl: "", artist: "Stale Local Artist", title: "Fresh", favorite: false, localAvailability: "available", provenance: {} };
    const loadPage = vi.fn((request: { query: string }) => new Promise<{ items: typeof fresh[]; total: number; limit: number; offset: number }>(resolve => pending.push({ request, resolve })));
    const target = render({ initialArtistRef: ref.artist, initialSection: "local", getArtistProfile: async () => profile(ref), onLoadLocalAssetsPage: loadPage });
    await settle();
    const query = target.querySelector<HTMLInputElement>('[aria-label="Filter local artist recordings"]')!;
    query.value = "fresh"; query.dispatchEvent(new Event("input", { bubbles: true }));
    await settle();
    expect(loadPage).toHaveBeenCalledTimes(2);
    expect(loadPage).toHaveBeenLastCalledWith(expect.objectContaining({ query: "fresh", offset: 0 }));
    pending[1].resolve({ items: [fresh], total: 1, limit: 200, offset: 0 });
    await settle();
    pending[0].resolve({ items: [old], total: 1, limit: 200, offset: 0 });
    await settle();
    expect(target.querySelector('[aria-label="Local artist recordings"]')?.textContent).toContain("Fresh");
    expect(target.querySelector('[aria-label="Local artist recordings"]')?.textContent).not.toContain("Old");
  });

  it("clears stale artist pages when a new search starts", async () => {
    let finishOld!: (value: { query: string; page: number; pageSize: number; hasMore: boolean; results: ArtistSearchResult[]; warnings: string[] }) => void;
    const old = artist("old", "Old Artist");
    const fresh = artist("fresh", "Fresh Artist");
    const searchArtists = vi.fn((query: string, page: number) => query === "old"
      ? new Promise<{ query: string; page: number; pageSize: number; hasMore: boolean; results: ArtistSearchResult[]; warnings: string[] }>(resolve => { finishOld = resolve; })
      : Promise.resolve({ query, page, pageSize: 25, hasMore: false, results: [fresh], warnings: [] }));
    const target = render({ artistName: "old", searchArtists });
    await settle();
    const input = target.querySelector<HTMLInputElement>("[aria-label='Search artist name']")!;
    input.value = "fresh"; input.dispatchEvent(new Event("input", { bubbles: true }));
    target.querySelector(".search")!.dispatchEvent(new Event("submit", { bubbles: true, cancelable: true }));
    await settle();
    expect(target.textContent).toContain("Fresh Artist");
    expect(target.textContent).not.toContain("Old Artist");
    finishOld({ query: "old", page: 1, pageSize: 25, hasMore: true, results: [old], warnings: [] });
    await settle();
    expect(target.textContent).not.toContain("Old Artist");
    expect(target.querySelector<HTMLButtonElement>("button")?.textContent).not.toContain("Load more artists");
  });
  it("deduplicates repeated release identities across pages while keeping master and release URLs distinct", async () => {
    const first = release("same", "Release", "https://musicbrainz.org/release/same", "Album");
    const target = render({ initialArtistRef: { provider: "musicbrainz", id: "artist-1" }, initialSection: "releases",
      getArtistProfile: async () => profile(artist("artist-1"), emptyReleasePage("artist-1", [first], "next")),
      getArtistReleases: async () => emptyReleasePage("artist-1", [first, { ...first, sourceUrl: "https://musicbrainz.org/release-group/same" }]),
    });
    await settle();
    [...target.querySelectorAll<HTMLButtonElement>("button")].find((button) => button.textContent?.includes("Load more releases"))?.click();
    await settle();
    expect(target.querySelectorAll(".release-list article").length).toBe(1);
    expect(target.querySelector<HTMLSelectElement>('[aria-label="Edition of Release"]')?.options.length).toBe(2);
  });

  it("shows album rows under the default Albums filter and opens source with no active track", async () => {
    const album = release("album-1", "Dragon New Warm Mountain", "https://musicbrainz.org/release-group/album-1", "Album");
    const ep = release("ep-1", "Little Things", "https://musicbrainz.org/release-group/ep-1", "EP");
    const onScroll = vi.fn();
    const target = render({ initialArtistRef: { provider: "musicbrainz", id: "artist-2" }, initialSection: "releases", initialScroll: 46, onScroll,
      getArtistProfile: async () => profile(artist("artist-2"), emptyReleasePage("artist-2", [album, ep])),
    });
    await settle();
    const page = target.querySelector<HTMLElement>(".artist-page")!;
    expect(page.scrollTop).toBe(46);
    page.dispatchEvent(new Event("scroll"));
    expect(onScroll).toHaveBeenCalledWith(46);
    expect(target.textContent).toContain("Dragon New Warm Mountain");
    expect(target.textContent).not.toContain("Little Things");
    expect(target.querySelector<HTMLAnchorElement>('a[href="https://musicbrainz.org/release-group/album-1"]')?.textContent).toContain("Open edition source");
    expect(target.querySelector<HTMLButtonElement>(".release-actions button")?.disabled).toBe(true);
  });

  it("ignores a stale profile response and preserves the requested section", async () => {
    let finishOld!: (value: ArtistProfile) => void;
    const old = artist("old", "Old Artist");
    const current = artist("current", "Current Artist");
    const target = render({ artistName: "artist", searchArtists: async () => ({ query: "artist", page: 1, pageSize: 25, hasMore: false, results: [old, current], warnings: [] }),
      initialSection: "releases",
      getArtistProfile: (ref: { id: string }) => ref.id === "old" ? new Promise<ArtistProfile>((resolve) => { finishOld = resolve; }) : Promise.resolve(profile(current)),
    });
    await settle();
    const resultButtons = target.querySelectorAll<HTMLButtonElement>(".artist-result");
    resultButtons[0].click(); resultButtons[1].click();
    await settle();
    finishOld(profile(old)); await settle();
    expect(target.querySelector(".artist-heading h2")?.textContent).toBe("Current Artist");
    expect(target.querySelector(".tabs button.active")?.textContent).toBe("Discography");
  });

  it("plays the full selected edition in order and starts a clicked track at its album offset", async () => {
    const ref = artist("playable", "Playable Artist");
    const album = release("album", "First Record", "https://musicbrainz.org/release/album", "Album");
    const songs = ["One", "Two", "Three"].map((title, index) => ({ id: `song-${index}`, provider: "musicbrainz", providerId: `song-${index}`, sourceUrl: "", artist: "Playable Artist", title, favorite: false, provenance: {} }));
    const onPlayTrack = vi.fn();
    const target = render({ initialArtistRef: ref.artist, initialSection: "releases", onPlayTrack,
      getArtistProfile: async () => profile(ref, emptyReleasePage(ref.artist.id, [album])),
      getReleaseTracks: async () => songs,
    });
    await settle();
    const buttons = [...target.querySelectorAll<HTMLButtonElement>(".release-actions button")];
    buttons.find((button) => button.textContent === "Play album")!.click(); await settle();
    expect(onPlayTrack).toHaveBeenLastCalledWith(songs[0], songs, 0);
    buttons.find((button) => button.textContent === "Tracks")!.click(); await settle();
    target.querySelectorAll<HTMLButtonElement>(".expanded-tracks .track-title")[2].click(); await settle();
    expect(onPlayTrack).toHaveBeenLastCalledWith(songs[2], songs, 2);
  });

  it("filters and sorts expanded release tracks while preserving album playback order and duplicate occurrences", async () => {
    const ref = artist("sortable", "Sortable Artist");
    const album = release("album", "Sortable Record", "https://musicbrainz.org/release/sortable", "Album");
    const alpha = { id: "one", provider: "musicbrainz", providerId: "one", sourceUrl: "", artist: "Sortable Artist", title: "Alpha", durationSeconds: 120, favorite: false, provenance: {} };
    const beta = { id: "two", provider: "musicbrainz", providerId: "two", sourceUrl: "", artist: "Sortable Artist", title: "Beta", durationSeconds: 60, favorite: false, provenance: {} };
    const shortAlpha = { id: "short", provider: "musicbrainz", providerId: "short", sourceUrl: "", artist: "Sortable Artist", title: "Alpha", durationSeconds: 30, favorite: false, provenance: {} };
    const songs = [alpha, beta, alpha, shortAlpha];
    const onPlayTrack = vi.fn();
    const target = render({ initialArtistRef: ref.artist, initialSection: "releases", onPlayTrack,
      getArtistProfile: async () => profile(ref, emptyReleasePage(ref.artist.id, [album])),
      getReleaseTracks: async () => songs,
    });
    await settle();
    [...target.querySelectorAll<HTMLButtonElement>(".release-actions button")].find(button => button.textContent === "Tracks")!.click();
    await settle();
    const sort = target.querySelector<HTMLSelectElement>('[aria-label="Sort loaded release tracks"]')!;
    sort.value = "title"; sort.dispatchEvent(new Event("change", { bubbles: true }));
    const minimum = target.querySelector<HTMLInputElement>('[aria-label="Minimum loaded release track duration"]')!;
    minimum.value = "60"; minimum.dispatchEvent(new Event("input", { bubbles: true }));
    await settle();
    const visibleTitles = [...target.querySelectorAll<HTMLButtonElement>(".expanded-tracks .track-title")];
    expect(visibleTitles.map(button => button.textContent?.trim())).toEqual(["Alpha", "Alpha", "Beta"]);
    visibleTitles[1].click();
    await settle();
    expect(onPlayTrack).toHaveBeenLastCalledWith(alpha, songs, 2);
    expect(songs.map(track => track.id)).toEqual(["one", "two", "one", "short"]);
  });

  it("keeps original source occurrence indices when descending sort reverses a repeated reference", async () => {
    const ref = artist("descending-occurrences", "Descending Occurrences");
    const album = release("album", "Repeated Record", "https://musicbrainz.org/release/repeated", "Album");
    const repeated = { id: "repeat", provider: "musicbrainz", providerId: "repeat", sourceUrl: "", artist: ref.name, title: "Same", favorite: false, provenance: {} };
    const middle = { id: "middle", provider: "musicbrainz", providerId: "middle", sourceUrl: "", artist: ref.name, title: "Middle", favorite: false, provenance: {} };
    const songs = [repeated, middle, repeated];
    const onPlayTrack = vi.fn();
    const target = render({ initialArtistRef: ref.artist, initialSection: "releases", onPlayTrack,
      getArtistProfile: async () => profile(ref, emptyReleasePage(ref.artist.id, [album])),
      getReleaseTracks: async () => songs,
    });
    await settle();
    [...target.querySelectorAll<HTMLButtonElement>(".release-actions button")].find(button => button.textContent === "Tracks")!.click();
    await settle();
    const sort = target.querySelector<HTMLSelectElement>('[aria-label="Sort loaded release tracks"]')!;
    sort.value = "sourceOrder"; sort.dispatchEvent(new Event("change", { bubbles: true }));
    const direction = target.querySelector<HTMLSelectElement>('[aria-label="Release track sort direction"]')!;
    direction.value = "desc"; direction.dispatchEvent(new Event("change", { bubbles: true }));
    await settle();
    expect([...target.querySelectorAll<HTMLElement>(".expanded-tracks .track-position")].map(node => node.textContent?.trim())).toEqual(["3", "2", "1"]);
    target.querySelector<HTMLButtonElement>('.expanded-tracks [aria-label="Play Same"]')!.click();
    await settle();
    expect(onPlayTrack).toHaveBeenLastCalledWith(repeated, songs, 2);
  });

  it("queues the selected edition without starting playback", async () => {
    const ref = artist("queueable", "Queueable Artist");
    const album = release("album", "Queue Record", "https://musicbrainz.org/release/queue", "Album");
    const songs = ["A", "B"].map((title, index) => ({ id: `queue-${index}`, provider: "musicbrainz", providerId: `queue-${index}`, sourceUrl: "", artist: "Queueable Artist", title, favorite: false, provenance: {} }));
    const onPlayTrack = vi.fn(); const onQueueTracks = vi.fn();
    const target = render({ initialArtistRef: ref.artist, initialSection: "releases", onPlayTrack, onQueueTracks,
      getArtistProfile: async () => profile(ref, emptyReleasePage(ref.artist.id, [album])), getReleaseTracks: async () => songs,
    });
    await settle();
    [...target.querySelectorAll<HTMLButtonElement>(".release-actions button")].find((button) => button.textContent === "Queue")!.click(); await settle();
    expect(onQueueTracks).toHaveBeenCalledWith(songs, "append");
    expect(onPlayTrack).not.toHaveBeenCalled();
  });

  it("filters loaded tracks and plays in the displayed sorted context", async () => {
    const ref = artist("tracks", "Track Artist");
    const songs = [
      { id: "b", provider: "musicbrainz", providerId: "b", sourceUrl: "", artist: "Track Artist", title: "Zulu", album: "Blue Record", releaseDate: "2001", durationSeconds: 60, favorite: false, provenance: {} },
      { id: "a", provider: "musicbrainz", providerId: "a", sourceUrl: "", artist: "Track Artist", title: "Alpha", album: "Blue Record", releaseDate: "2002", durationSeconds: 180, favorite: false, provenance: {} },
      { id: "c", provider: "musicbrainz", providerId: "c", sourceUrl: "", artist: "Track Artist", title: "Gamma", album: "Other Record", durationSeconds: 300, favorite: false, provenance: {} },
    ];
    const onPlayTrack = vi.fn();
    const target = render({ initialArtistRef: ref.artist, initialSection: "overview", onPlayTrack,
      getArtistProfile: async () => profile(ref), getArtistTracks: async () => ({ artist: ref.artist, items: songs, source: "MusicBrainz", warnings: [] }),
    });
    await settle();
    target.querySelector<HTMLButtonElement>('.tabs button:nth-child(3)')!.click(); await settle();
    target.querySelector<HTMLSelectElement>('[aria-label="Sort tracks"]')!.value = "title";
    target.querySelector<HTMLSelectElement>('[aria-label="Sort tracks"]')!.dispatchEvent(new Event("change", { bubbles: true })); await settle();
    const rows = target.querySelectorAll(".shared-track-list li");
    expect(rows[0].textContent).toContain("Alpha");
    rows[0].querySelector<HTMLButtonElement>("button")!.click(); await settle();
    expect(onPlayTrack).toHaveBeenLastCalledWith(songs[1], [songs[1], songs[2], songs[0]], 0);
    const search = target.querySelector<HTMLInputElement>('[aria-label="Search loaded tracks"]')!;
    search.value = "Blue Record"; search.dispatchEvent(new Event("input", { bubbles: true })); await settle();
    expect(target.querySelectorAll(".shared-track-list li")).toHaveLength(2);
    expect(target.querySelector(".shared-track-list")?.textContent).toContain("Zulu");
    expect(target.querySelector(".shared-track-list")?.textContent).toContain("Alpha");
    expect(target.querySelector(".shared-track-list")?.textContent).not.toContain("Gamma");
    const minimumDuration = target.querySelector<HTMLInputElement>('[aria-label="Minimum track duration"]')!;
    minimumDuration.value = "2:00"; minimumDuration.dispatchEvent(new Event("input", { bubbles: true })); await settle();
    expect(target.querySelectorAll(".shared-track-list li")).toHaveLength(1);
    expect(target.querySelector(".shared-track-list")?.textContent).toContain("Alpha");
    expect(target.querySelector(".shared-track-list")?.textContent).not.toContain("Zulu");
    expect(target.querySelector(".shared-track-list")?.textContent).not.toContain("Gamma");
    search.value = ""; search.dispatchEvent(new Event("input", { bubbles: true })); await settle();
    target.querySelector<HTMLSelectElement>('[aria-label="Sort tracks"]')!.value = "duration";
    target.querySelector<HTMLSelectElement>('[aria-label="Sort tracks"]')!.dispatchEvent(new Event("change", { bubbles: true })); await settle();
    target.querySelector<HTMLSelectElement>('[aria-label="Sort direction"]')!.value = "desc";
    target.querySelector<HTMLSelectElement>('[aria-label="Sort direction"]')!.dispatchEvent(new Event("change", { bubbles: true })); await settle();
    expect(target.querySelector(".track-title")?.textContent).toContain("Gamma");
  });

  it("keeps artist source sort fields enabled across filters and unfinished pages", async () => {
    const ref = artist("source-sort-availability", "Source Sort Artist");
    const songs = [
      { id: "has-bpm", provider: "musicbrainz", providerId: "has-bpm", sourceUrl: "", artist: "Source Sort Artist", title: "Zulu BPM", bpm: 120, durationSeconds: 180, favorite: false, provenance: {} },
      { id: "no-bpm", provider: "musicbrainz", providerId: "no-bpm", sourceUrl: "", artist: "Source Sort Artist", title: "Alpha", durationSeconds: 180, favorite: false, provenance: {} },
    ];
    const target = render({ initialArtistRef: ref.artist, getArtistProfile: async () => profile(ref), getArtistTracks: async () => ({ artist: ref.artist, items: songs, source: "MusicBrainz", warnings: [] }) });
    await settle(); target.querySelector<HTMLButtonElement>('.tabs button:nth-child(3)')!.click(); await settle();
    const sort = target.querySelector<HTMLSelectElement>('[aria-label="Sort tracks"]')!;
    const query = target.querySelector<HTMLInputElement>('[aria-label="Search loaded tracks"]')!;
    query.value = "Alpha"; query.dispatchEvent(new Event("input", { bubbles: true })); await settle();
    expect(target.querySelectorAll(".shared-track-list li")).toHaveLength(1);
    expect(sort.querySelector<HTMLOptionElement>('option[value="bpm"]')?.disabled).toBe(false);
    destroy?.(); destroy = undefined;

    const paged = render({ initialArtistRef: ref.artist, getArtistProfile: async () => profile(ref), getArtistTracks: async () => ({ artist: ref.artist, items: [songs[1]], source: "MusicBrainz", nextCursor: "next", warnings: [] }) });
    await settle(); paged.querySelector<HTMLButtonElement>('.tabs button:nth-child(3)')!.click(); await settle();
    expect(paged.querySelector<HTMLSelectElement>('[aria-label="Sort tracks"]')?.querySelector<HTMLOptionElement>('option[value="bpm"]')?.disabled).toBe(false);
  });

  it("ignores a pending album play after navigating to another artist", async () => {
    const old = artist("old-artist", "Old Artist"); const next = artist("new-artist", "New Artist");
    const album = release("old-album", "Old Record", "https://musicbrainz.org/release/old-album", "Album");
    let finishTracks!: (tracks: import("./lib/types").Track[]) => void;
    const songs = [{ id: "old-song", provider: "musicbrainz", providerId: "old-song", sourceUrl: "", artist: "Old Artist", title: "Late Result", favorite: false, provenance: {} }];
    const onPlayTrack = vi.fn();
    const target = render({ artistName: "Artist", initialSection: "releases", onPlayTrack,
      searchArtists: async () => ({ query: "Artist", page: 1, pageSize: 25, hasMore: false, results: [old, next], warnings: [] }),
      getArtistProfile: async (ref: { id: string }) => profile(ref.id === old.artist.id ? old : next, emptyReleasePage(ref.id, ref.id === old.artist.id ? [album] : [])),
      getReleaseTracks: () => new Promise((resolve) => { finishTracks = resolve; }),
    });
    await settle(); target.querySelector<HTMLButtonElement>(".artist-result")!.click(); await settle();
    [...target.querySelectorAll<HTMLButtonElement>(".release-actions button")].find((button) => button.textContent === "Play album")!.click();
    target.querySelector<HTMLButtonElement>(".back")!.click(); await settle();
    [...target.querySelectorAll<HTMLButtonElement>(".artist-result")].find((button) => button.textContent?.includes("New Artist"))!.click(); await settle();
    finishTracks(songs); await settle();
    expect(target.querySelector(".artist-heading h2")?.textContent).toBe("New Artist");
    expect(onPlayTrack).not.toHaveBeenCalled();
  });

  it("keeps concurrent edition track loads independent and cached", async () => {
    const ref = artist("editions", "Edition Artist");
    const first = release("first", "Shared Album", "https://musicbrainz.org/release/first", "Album");
    first.year = 2002;
    const second = { ...release("second", "Shared Album", "https://musicbrainz.org/release/second", "Album"), year: 2010 };
    const results: Record<string, (tracks: import("./lib/types").Track[]) => void> = {};
    const trackFor = (id: string, title: string) => ({ id, provider: "musicbrainz", providerId: id, sourceUrl: "", artist: "Edition Artist", title, favorite: false, provenance: {} });
    const target = render({ initialArtistRef: ref.artist, initialSection: "releases",
      getArtistProfile: async () => profile(ref, emptyReleasePage(ref.artist.id, [first, second])),
      getReleaseTracks: (edition: ArtistReleaseRef) => new Promise((resolve) => { results[edition.sourceUrl] = resolve; }),
    });
    await settle();
    const card = target.querySelector(".release-list article")!;
    const editionSelect = target.querySelector<HTMLSelectElement>('[aria-label="Edition of Shared Album"]')!;
    editionSelect.value = first.sourceUrl; editionSelect.dispatchEvent(new Event("change", { bubbles: true })); await settle();
    card.querySelector<HTMLButtonElement>('.release-actions button[aria-expanded="false"]')!.click();
    await settle();
    editionSelect.value = second.sourceUrl; editionSelect.dispatchEvent(new Event("change", { bubbles: true })); await settle();
    expect(results[first.sourceUrl]).toBeDefined(); expect(results[second.sourceUrl]).toBeDefined();
    results[first.sourceUrl]([trackFor("first-song", "First Edition Song")]); await settle();
    results[second.sourceUrl]([trackFor("second-song", "Second Edition Song")]); await settle();
    editionSelect.value = first.sourceUrl; editionSelect.dispatchEvent(new Event("change", { bubbles: true })); await settle();
    expect(card.textContent).toContain("First Edition Song");
    editionSelect.value = second.sourceUrl; editionSelect.dispatchEvent(new Event("change", { bubbles: true })); await settle();
    expect(card.textContent).toContain("Second Edition Song");
  });

  it("sorts album groups by their earliest known edition year", async () => {
    const ref = artist("chronology", "Chronology Artist");
    const albumA = release("a", "Early Record", "https://musicbrainz.org/release/a", "Album"); albumA.year = 2012;
    const albumAReissue = release("a-reissue", "Early Record", "https://musicbrainz.org/release/a-reissue", "Album"); albumAReissue.year = 2024;
    const albumB = release("b", "Later Record", "https://musicbrainz.org/release/b", "Album"); albumB.year = 2016;
    const target = render({ initialArtistRef: ref.artist, initialSection: "releases", getArtistProfile: async () => profile(ref, emptyReleasePage(ref.artist.id, [albumB, albumAReissue, albumA])) });
    await settle();
    expect([...target.querySelectorAll(".release-info strong")].map((node) => node.textContent)).toEqual(["Later Record", "Early Record"]);
    target.querySelector<HTMLSelectElement>('[aria-label="Sort releases"]')!.value = "oldest";
    target.querySelector<HTMLSelectElement>('[aria-label="Sort releases"]')!.dispatchEvent(new Event("change", { bubbles: true })); await settle();
    expect([...target.querySelectorAll(".release-info strong")].map((node) => node.textContent)).toEqual(["Early Record", "Later Record"]);
    expect(target.querySelector(".catalog-count")?.textContent).toContain("2 albums · 3 editions loaded");
  });

  it("downloads the complete selected edition after loading its tracks", async () => {
    const ref = artist("download", "Download Artist");
    const album = release("album-download", "Download Record", "https://musicbrainz.org/release/download", "Album");
    const songs: Track[] = [
      { id: "d2", provider: "musicbrainz", providerId: "d2", sourceUrl: "", artist: "Download Artist", title: "Second", favorite: false, provenance: { position: "2" } },
      { id: "d1", provider: "musicbrainz", providerId: "d1", sourceUrl: "", artist: "Download Artist", title: "First", favorite: false, provenance: { position: "1" } },
    ];
    const onDownloadAlbum = vi.fn();
    const target = render({ initialArtistRef: ref.artist, initialSection: "releases", onDownloadAlbum,
      getArtistProfile: async () => profile(ref, emptyReleasePage(ref.artist.id, [album])),
      getReleaseTracks: async () => songs,
    });
    await settle();
    target.querySelector<HTMLButtonElement>('[aria-label="Download album Download Record"]')!.click();
    await settle();
    expect(onDownloadAlbum).toHaveBeenCalledWith(songs, album);
  });

  it("saves the full source-order album while its expanded track list is filtered", async () => {
    const ref = artist("save-library", "Save Artist");
    const album = release("save-album", "Save Record", "https://musicbrainz.org/release/save", "Album");
    album.trackCount = 4;
    const songs: Track[] = ["First", "Second", "Third"].map((title, index) => ({
      id: `save-${index}`, provider: "musicbrainz", providerId: `save-${index}`, sourceUrl: "",
      artist: "Save Artist", title, favorite: false, provenance: { position: String(index + 1) },
    }));
    const onSaveAlbumToLibrary = vi.fn(async () => {});
    const onPlayTrack = vi.fn();
    const onQueueTracks = vi.fn();
    const target = render({ initialArtistRef: ref.artist, initialSection: "releases", onSaveAlbumToLibrary, onPlayTrack, onQueueTracks,
      getArtistProfile: async () => profile(ref, emptyReleasePage(ref.artist.id, [album])),
      getReleaseTracks: async () => songs,
    });
    await settle();
    target.querySelector<HTMLButtonElement>('.release-actions button[aria-expanded="false"]')?.click();
    await settle();
    const filter = target.querySelector<HTMLInputElement>('[aria-label="Filter loaded release tracks"]')!;
    filter.value = "Second"; filter.dispatchEvent(new Event("input", { bubbles: true })); await settle();
    expect(target.querySelector(".release-track-controls > span")?.textContent).toContain("1 of 3 loaded tracks");
    expect(target.querySelector(".expanded-tracks")?.textContent).toContain("Second");
    expect(target.querySelector(".expanded-tracks")?.textContent).not.toContain("First");
    target.querySelector<HTMLButtonElement>('[aria-label="Save album Save Record to library"]')!.click();
    await settle();
    expect(onSaveAlbumToLibrary).toHaveBeenCalledOnce();
    expect(onSaveAlbumToLibrary).toHaveBeenCalledWith(songs, album);
    expect(onPlayTrack).not.toHaveBeenCalled();
    expect(onQueueTracks).not.toHaveBeenCalled();
    expect(target.querySelector(".album-save-status")?.textContent).toContain("3 of 4 source tracks available");
  });

  it("ignores duplicate and stale save requests when the selected edition changes", async () => {
    const ref = artist("save-stale", "Save Artist");
    const first = release("save-first", "Shared Save", "https://musicbrainz.org/release/save-first", "Album"); first.year = 2002;
    const second = { ...release("save-second", "Shared Save", "https://musicbrainz.org/release/save-second", "Album"), year: 2010 };
    let finishLoad!: (tracks: Track[]) => void;
    const getReleaseTracks = vi.fn(() => new Promise<Track[]>(resolve => { finishLoad = resolve; }));
    const onSaveAlbumToLibrary = vi.fn(async () => {});
    const target = render({ initialArtistRef: ref.artist, initialSection: "releases", onSaveAlbumToLibrary,
      getArtistProfile: async () => profile(ref, emptyReleasePage(ref.artist.id, [first, second])), getReleaseTracks,
    });
    await settle();
    const saveButton = target.querySelector<HTMLButtonElement>('[aria-label="Save album Shared Save to library"]')!;
    saveButton.click();
    saveButton.click();
    const edition = target.querySelector<HTMLSelectElement>('[aria-label="Edition of Shared Save"]')!;
    edition.value = first.sourceUrl; edition.dispatchEvent(new Event("change", { bubbles: true })); await settle();
    finishLoad([{ id: "stale", provider: "musicbrainz", providerId: "stale", sourceUrl: "", artist: "Save Artist", title: "Stale", favorite: false, provenance: {} }]);
    await settle();
    expect(getReleaseTracks).toHaveBeenCalledTimes(1);
    expect(onSaveAlbumToLibrary).not.toHaveBeenCalled();
    expect(target.querySelector('[aria-label="Save album Shared Save to library"]')?.textContent).toContain("Save to library");
  });
});

describe("catalog artist history snapshots",()=>{
  it("restores expanded release filters and duration controls per history entry",async()=>{
    const ref=artist("snapshot-artist");const album=release("snapshot-release","Snapshot Release","https://musicbrainz.org/release/snapshot-release","Album");
    const songs=["One","Two"].map((title,index)=>({id:`song-${index}`,provider:"musicbrainz",providerId:`song-${index}`,sourceUrl:"",artist:ref.name,title,durationSeconds:180,favorite:false,provenance:{}}));
    const target=render({initialArtistRef:ref.artist,initialSection:"releases",getArtistProfile:async()=>profile(ref,emptyReleasePage(ref.artist.id,[album])),getReleaseTracks:async()=>songs});await settle();
    [...target.querySelectorAll<HTMLButtonElement>(".release-actions button")].find(button=>button.textContent==="Tracks")!.click();await settle();
    const filter=target.querySelector<HTMLInputElement>('[aria-label="Filter loaded release tracks"]')!;
    filter.value="Two";filter.dispatchEvent(new Event("input",{bubbles:true}));await settle();snapshotInstance.captureNavigationSnapshot("catalog-a");
    filter.value="One";filter.dispatchEvent(new Event("input",{bubbles:true}));await settle();snapshotInstance.captureNavigationSnapshot("catalog-b");
    await snapshotInstance.restoreNavigationSnapshot("catalog-a");await settle();
    expect(target.querySelector<HTMLInputElement>('[aria-label="Filter loaded release tracks"]')?.value).toBe("Two");
    expect(target.querySelector(".expanded-tracks")?.textContent).toContain("Two");
    expect(target.querySelector(".expanded-tracks")?.textContent).not.toContain("One");
    await snapshotInstance.restoreNavigationSnapshot("catalog-b");await settle();
    expect(target.querySelector<HTMLInputElement>('[aria-label="Filter loaded release tracks"]')?.value).toBe("One");
    providerNavigationSnapshots.put({kind:"artist-page",key:"eviction-pressure",accountScope:"public-catalog",snapshot:{},rows:20001,stripRows:()=>({})});
    await snapshotInstance.restoreNavigationSnapshot("catalog-a");await settle();
    expect(target.querySelector<HTMLInputElement>('[aria-label="Filter loaded release tracks"]')?.value).toBe("Two");
    expect(target.querySelector(".expanded-tracks")?.textContent).toContain("Two");
    providerNavigationSnapshots.clear();
  });
});
