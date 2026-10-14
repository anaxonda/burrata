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
      { id: "b", provider: "musicbrainz", providerId: "b", sourceUrl: "", artist: "Track Artist", title: "Zulu", releaseDate: "2001", favorite: false, provenance: {} },
      { id: "a", provider: "musicbrainz", providerId: "a", sourceUrl: "", artist: "Track Artist", title: "Alpha", releaseDate: "2002", favorite: false, provenance: {} },
    ];
    const onPlayTrack = vi.fn();
    const target = render({ initialArtistRef: ref.artist, initialSection: "overview", onPlayTrack,
      getArtistProfile: async () => profile(ref), getArtistTracks: async () => ({ artist: ref.artist, items: songs, source: "MusicBrainz", warnings: [] }),
    });
    await settle();
    target.querySelector<HTMLButtonElement>('.tabs button:nth-child(3)')!.click(); await settle();
    target.querySelector<HTMLSelectElement>('[aria-label="Sort tracks"]')!.value = "title";
    target.querySelector<HTMLSelectElement>('[aria-label="Sort tracks"]')!.dispatchEvent(new Event("change", { bubbles: true })); await settle();
    const rows = target.querySelectorAll(".track-list li");
    expect(rows[0].textContent).toContain("Alpha");
    rows[0].querySelector<HTMLButtonElement>("button")!.click(); await settle();
    expect(onPlayTrack).toHaveBeenLastCalledWith(songs[1], [songs[1], songs[0]], 0);
    const search = target.querySelector<HTMLInputElement>('[aria-label="Search loaded tracks"]')!;
    search.value = "Zulu"; search.dispatchEvent(new Event("input", { bubbles: true })); await settle();
    expect(target.querySelectorAll(".track-list li")).toHaveLength(1);
    expect(target.querySelector(".track-list")?.textContent).toContain("Zulu");
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
});
