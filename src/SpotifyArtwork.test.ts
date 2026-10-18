// @vitest-environment jsdom
// @ts-expect-error Exercise Svelte's browser runtime in jsdom.
import { mount, tick, unmount } from "../node_modules/svelte/src/index-client.js";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
// @ts-expect-error Match the browser runtime used by the direct import above.
vi.mock("svelte", async () => await import("../node_modules/svelte/src/index-client.js"));
import RemotePlaylists from "./RemotePlaylists.svelte";
import SearchPanel from "./SearchPanel.svelte";
import SpotifyArtist from "./SpotifyArtist.svelte";
import { safeArtworkUrl, safeArtworkUrls } from "./lib/artwork-url";
import { providerNavigationSnapshots } from "./lib/provider-navigation-snapshots";

let destroy: (() => void) | undefined;
beforeEach(() => {
  providerNavigationSnapshots.clear();
  vi.stubGlobal("requestAnimationFrame", (callback: FrameRequestCallback) => { callback(0); return 1; });
});
afterEach(() => { vi.unstubAllGlobals(); destroy?.(); destroy = undefined; document.body.innerHTML = ""; vi.restoreAllMocks(); });
async function settle() { for (let i = 0; i < 10; i++) await Promise.resolve(); await tick(); }
function mountComponent(component: unknown, props: Record<string, unknown>) {
  const target = document.createElement("div"); document.body.append(target);
  const instance = mount(component as never, { target, props: props as never });
  destroy = () => unmount(instance);
  return target;
}

describe("Spotify artwork visibility", () => {
  it("accepts only bounded credential-free HTTPS artwork URLs", () => {
    expect(safeArtworkUrl("https://i.scdn.co/image/fixture")).toBe("https://i.scdn.co/image/fixture");
    expect(safeArtworkUrl("http://i.scdn.co/image/fixture")).toBeNull();
    expect(safeArtworkUrl("https://user:pass@i.scdn.co/image/fixture")).toBeNull();
    expect(safeArtworkUrl(`https://i.scdn.co/${"x".repeat(2048)}`)).toBeNull();
    expect(safeArtworkUrls(["https://a.example/1", "https://a.example/1", "https://b.example/2"], 2)).toEqual([
      "https://a.example/1", "https://b.example/2",
    ]);
  });

  it("shows a Spotify artist image and advances to a release cover if it fails", async () => {
    const id = "spotify-artist-art";
    const artistArt = "https://i.scdn.co/image/artist-art";
    const releaseArt = "https://i.scdn.co/image/release-art";
    const album = { id: "release-id", title: "Release", url: "https://open.spotify.com/album/release-id", artworkUrl: releaseArt, artists: ["Fixture Artist"] };
    const call = vi.fn(async (command: string) => {
      if (command === "browse_remote_artist") return { artist: { id, name: "Fixture Artist", url: `https://open.spotify.com/artist/${id}`, artworkUrl: artistArt, genres: [] }, albums: [album] };
      if (command === "browse_remote_artist_tracks") return { provider: "spotify", artistId: id, tracks: [], total: 0 };
      throw new Error(`Unexpected command ${command}`);
    });
    const target = mountComponent(SpotifyArtist, { artistId: id, call });
    await settle();
    const image = target.querySelector<HTMLImageElement>(".hero img")!;
    expect(image.src).toBe(artistArt);
    image.dispatchEvent(new Event("error")); await settle();
    expect(target.querySelector<HTMLImageElement>(".hero img")?.src).toBe(releaseArt);
  });

  it("renders artwork on Spotify profile search choices", async () => {
    const artworkUrl = "https://i.scdn.co/image/profile-art";
    const call = vi.fn(async (command: string) => {
      if (command === "get_managed_provider_session_status") return { spotifyAccountId: "fixture-account" };
      if (command === "list_remote_playlist_bindings") return [];
      if (command === "list_remote_playlists") return { items: [], nextCursor: null };
      if (command === "search_source") return { results: [{ id: "profile-art-id", kind: "profile", title: "Fixture profile", subtitle: "Spotify profile", url: "https://open.spotify.com/user/profile-art-id", artworkUrl }] };
      throw new Error(`Unexpected command ${command}`);
    });
    const target = mountComponent(RemotePlaylists, { call, initialOwner: "Fixture profile" });
    await settle();
    const search = [...target.querySelectorAll<HTMLButtonElement>("button")].find(button => button.textContent?.includes("Search profiles"))!;
    search.click(); await settle();
    expect(target.querySelector<HTMLImageElement>(".remote-profile-choices img")?.src).toBe(artworkUrl);
    target.querySelector<HTMLImageElement>(".remote-profile-choices img")!.dispatchEvent(new Event("error"));
    expect(target.querySelector(".remote-profile-choices .profile-art > span")?.textContent).toBe("♫");
  });

  it("renders Spotify artist and profile artwork in online search results", async () => {
    const artistArt = "https://i.scdn.co/image/search-artist";
    const profileArt = "https://i.scdn.co/image/search-profile";
    const rows = [
      { id: "artist-search", kind: "artist", sourceId: "spotify", title: "Search Artist", artworkUrl: artistArt, url: "https://open.spotify.com/artist/artist-search" },
      { id: "profile-search", kind: "profile", sourceId: "spotify", title: "Search Profile", artworkUrl: profileArt, url: "https://open.spotify.com/user/profile-search" },
    ];
    const searchSource = vi.fn(async () => ({ sourceId: "spotify", complete: true, results: rows }));
    const target = mountComponent(SearchPanel, {
      initialSources: ["spotify"], availableSources: [{ id: "spotify", name: "Spotify", enabled: true, kinds: ["artist", "profile"] }], searchSource,
    });
    await settle();
    const input = target.querySelector<HTMLInputElement>("[aria-label='Search music']")!;
    input.value = "search"; input.dispatchEvent(new Event("input", { bubbles: true }));
    target.querySelector<HTMLFormElement>(".search-form")!.dispatchEvent(new Event("submit", { bubbles: true, cancelable: true })); await settle();
    expect([...target.querySelectorAll<HTMLImageElement>(".spotify-identity-art img")].map(image => image.src)).toEqual([artistArt, profileArt]);
  });
});
