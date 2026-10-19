// @vitest-environment jsdom
// @ts-expect-error Exercise Svelte's browser runtime in jsdom.
import { mount, tick, unmount } from "../node_modules/svelte/src/index-client.js";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
// @ts-expect-error Use Svelte's reactive props object to simulate a parent prop update.
import { proxy } from "../node_modules/svelte/src/internal/client/proxy.js";
// @ts-expect-error Match the browser runtime used by the direct import above.
vi.mock("svelte", async () => await import("../node_modules/svelte/src/index-client.js"));
import RemotePlaylistDetail from "./RemotePlaylistDetail.svelte";
import { providerNavigationSnapshots } from "./lib/provider-navigation-snapshots";
import type { Track } from "./lib/types";

let destroy: (() => void) | undefined;
beforeEach(() => {
  providerNavigationSnapshots.clear();
  vi.stubGlobal("requestAnimationFrame", (callback: FrameRequestCallback) => { callback(0); return 1; });
});
afterEach(() => { vi.useRealTimers(); vi.unstubAllGlobals(); unmountCurrent(); document.body.innerHTML = ""; });
async function settle() { for (let i = 0; i < 10; i++) await Promise.resolve(); await tick(); }
function deferred<T = unknown>() {
  let resolve!: (value: T) => void;
  let reject!: (reason?: unknown) => void;
  const promise = new Promise<T>((done, fail) => { resolve = done; reject = fail; });
  return { promise, resolve, reject };
}
function unmountCurrent() { const current = destroy; destroy = undefined; current?.(); }
const track = (id: string, title = id): Track => ({
  id, provider: "spotify", providerId: `spotify:${id}`, sourceUrl: `https://open.spotify.com/track/${id}`,
  artist: "Example Artist", title, favorite: false, provenance: { remoteEntryId: `entry-${id}`, availability: "unavailable" },
});
function render(call: (command: string, args?: Record<string, unknown>) => Promise<unknown>, extra: Record<string, unknown> = {}) {
  const target = document.createElement("div"); document.body.append(target);
  const instance = mount(RemotePlaylistDetail, { target, props: { provider: "spotify", playlistUrl: "https://open.spotify.com/playlist/remote-id", call, ...extra } });
  destroy = () => unmount(instance);
  return target;
}

describe("RemotePlaylistDetail", () => {
  it("distinguishes a confirmed empty playlist from a failed incomplete track load", async () => {
    const emptyCall = vi.fn(async (command: string) => {
      if (command === "browse_remote_playlist") return { provider: "youtube", playlistId: "yt-empty", title: "Empty playlist", tracks: [], total: 0, complete: true };
      throw new Error(`Unexpected command ${command}`);
    });
    const empty = render(emptyCall, { provider: "youtube", playlistUrl: "https://www.youtube.com/playlist?list=yt-empty" }); await settle();
    expect(empty.querySelector(".remote-detail-status")?.textContent).toContain("This playlist is empty.");
    expect(empty.querySelectorAll(".remote-detail-actions button")).toHaveLength(1);
    expect(empty.querySelector<HTMLButtonElement>(".remote-detail-retry")).toBeNull();

    unmountCurrent();
    const incompleteCall = vi.fn(async (command: string) => {
      if (command === "browse_remote_playlist") return { provider: "youtube", playlistId: "yt-incomplete", title: "Incomplete playlist", tracks: [], total: 62, complete: false };
      throw new Error(`Unexpected command ${command}`);
    });
    const incomplete = render(incompleteCall, { provider: "youtube", playlistUrl: "https://www.youtube.com/playlist?list=yt-incomplete" }); await settle();
    expect(incomplete.querySelector(".remote-detail-status")?.textContent).toContain("Couldn't load this playlist's tracks.");
    expect(incomplete.querySelector(".remote-detail-status")?.textContent).toContain("provider reported 62 tracks, but returned none");
    expect(incomplete.querySelectorAll(".remote-detail-actions button")).toHaveLength(1);
    const retry = incomplete.querySelector<HTMLButtonElement>(".remote-detail-retry");
    expect(retry?.textContent).toBe("Retry loading playlist");
    expect(retry?.classList.contains("primary")).toBe(true);
    retry!.click(); await settle();
    expect(incompleteCall.mock.calls.filter(([command]) => command === "browse_remote_playlist")).toHaveLength(2);
  });

  it("shows a browse failure without claiming that no tracks were returned, then recovers to a confirmed empty state", async () => {
    let attempts = 0;
    const call = vi.fn(async (command: string) => {
      if (command !== "browse_remote_playlist") throw new Error(`Unexpected command ${command}`);
      attempts += 1;
      if (attempts === 1) throw new Error("Spotify playlist request was denied");
      return { provider: "spotify", playlistId: "remote-id", title: "Remote playlist", tracks: [], total: 0, complete: true };
    });
    const target = render(call); await settle();

    expect(target.querySelector('[role="alert"]')?.textContent).toContain("Could not load playlist tracks: Error: Spotify playlist request was denied");
    expect(target.querySelector(".remote-detail-status")).toBeNull();
    const retry = target.querySelector<HTMLButtonElement>(".remote-detail-retry");
    expect(retry?.textContent).toBe("Retry loading playlist");
    expect(retry?.classList.contains("primary")).toBe(true);
    retry!.click(); await settle();
    expect(target.querySelector('[role="alert"]')).toBeNull();
    expect(target.querySelector(".remote-detail-status")?.textContent).toBe("This playlist is empty.");
  });

  it("keeps successfully loaded rows visible when an incomplete playlist reports more tracks", async () => {
    const rows = [track("partial-a", "Loaded track")];
    const call = vi.fn(async (command: string) => {
      if (command === "browse_remote_playlist") return { provider: "spotify", playlistId: "partial", title: "Partial playlist", tracks: rows, total: 3, complete: false };
      throw new Error(`Unexpected command ${command}`);
    });
    const target = render(call); await settle();
    expect(target.querySelectorAll(".shared-track-list li")).toHaveLength(1);
    expect(target.textContent).toContain("1 of 3 tracks loaded");
    expect(target.textContent).not.toContain("Couldn't load this playlist's tracks");
    expect(target.querySelector(".remote-detail-retry")?.textContent).toBe("Retry loading playlist");
    expect([...target.querySelectorAll<HTMLButtonElement>(".remote-detail-actions button")].map(button => button.textContent?.trim())).toEqual([
      "Play loaded tracks", "Queue loaded tracks", "Play loaded tracks next", "Add to library",
    ]);
  });

  it("routes a validated Spotify creator profile through the owner browser callback", async () => {
    const call = vi.fn(async (command: string) => {
      if (command === "browse_remote_playlist") return {
        provider: "spotify", playlistId: "remote-id", title: "Remote mix", ownerName: "Owner Zero",
        ownerUrl: "https://open.spotify.com/user/fixture-owner/", tracks: [], complete: true,
      };
      throw new Error(`Unexpected command ${command}`);
    });
    const onBrowseOwner = vi.fn();
    const target = render(call, { onBrowseOwner }); await settle();
    const owner = target.querySelector<HTMLButtonElement>('[aria-label="Browse Owner Zero playlists"]');
    expect(owner?.title).toBe("Browse Owner Zero playlists");
    owner!.click(); await settle();
    expect(onBrowseOwner).toHaveBeenCalledWith("spotify", "https://open.spotify.com/user/fixture-owner");
    expect(call).not.toHaveBeenCalledWith("open_external_url", expect.anything());
  });

  it("does not route unsupported Spotify owner paths and preserves YouTube external fallback", async () => {
    const spotifyCall = vi.fn(async (command: string) => {
      if (command === "browse_remote_playlist") return {
        provider: "spotify", playlistId: "remote-id", title: "Remote mix", ownerName: "Wrong route",
        ownerUrl: "https://open.spotify.com/user/joe%2Fadmin", tracks: [], complete: true,
      };
      throw new Error(`Unexpected command ${command}`);
    });
    const onBrowseOwner = vi.fn();
    const spotify = render(spotifyCall, { onBrowseOwner }); await settle();
    expect(spotify.querySelector(".remote-detail-owner")).toBeNull();
    expect(spotify.textContent).toContain("Wrong route");
    expect(onBrowseOwner).not.toHaveBeenCalled();

    unmountCurrent();
    const youtubeCall = vi.fn(async (command: string) => {
      if (command === "browse_remote_playlist") return {
        provider: "youtube", playlistId: "yt-id", title: "YouTube mix", ownerName: "Example Channel",
        ownerUrl: "https://www.youtube.com/@example", tracks: [], complete: true,
      };
      if (command === "open_external_url") return null;
      throw new Error(`Unexpected command ${command}`);
    });
    const youtube = render(youtubeCall, { provider: "youtube", playlistUrl: "https://www.youtube.com/playlist?list=yt-id", onBrowseOwner }); await settle();
    youtube.querySelector<HTMLButtonElement>('[aria-label="Open Example Channel profile in browser"]')!.click(); await settle();
    expect(youtubeCall).toHaveBeenCalledWith("open_external_url", { url: "https://www.youtube.com/@example" });
    expect(onBrowseOwner).not.toHaveBeenCalled();
  });

  it("browses playlist tracks without importing and forwards play/queue callbacks", async () => {
    const rows = [track("track-1", "First track"), track("track-2", "Second track")];
    const call = vi.fn(async (command: string) => {
      if (command === "browse_remote_playlist") return { provider: "spotify", playlistId: "remote-id", title: "Remote mix", tracks: rows, nextCursor: null, total: 2, complete: true };
      throw new Error(`Unexpected command ${command}`);
    });
    const onPlay = vi.fn(); const onQueueTracks = vi.fn();
    const target = render(call, { onPlay, onQueueTracks }); await settle();
    expect(call).toHaveBeenCalledWith("browse_remote_playlist", expect.objectContaining({ provider: "spotify", playlistId: "https://open.spotify.com/playlist/remote-id", requestId: expect.any(String) }));
    expect(call).not.toHaveBeenCalledWith("import_remote_playlists", expect.anything());
    expect(target.textContent).toContain("Remote mix");
    target.querySelector<HTMLButtonElement>('[aria-label="Play First track"]')!.click(); await settle();
    expect(onPlay).toHaveBeenCalledWith(rows[0], rows, 0);
    target.querySelector<HTMLButtonElement>('[aria-label="Play Second track"]')!.click(); await settle();
    expect(onPlay).toHaveBeenCalledWith(rows[1], rows, 1);
    [...target.querySelectorAll<HTMLButtonElement>("button")].find(button => button.textContent === "Play all tracks")!.click();
    expect(onPlay).toHaveBeenLastCalledWith(rows[0], rows, 0);
    [...target.querySelectorAll<HTMLButtonElement>("button")].find(button => button.textContent === "Queue all tracks")!.click();
    expect(onQueueTracks).toHaveBeenCalledWith(rows, "append");
    expect(target.textContent?.toLowerCase()).toContain("unavailable");
  });

  it("labels explicit provider unavailability without treating true or unknown as unavailable", async () => {
    const rows = [
      { ...track("provider-missing", "Provider missing"), provenance: { remoteEntryId: "entry-missing", available: false } },
      { ...track("provider-present", "Provider present"), provenance: { remoteEntryId: "entry-present", available: true } },
      { ...track("provider-unknown", "Provider unknown"), provenance: { remoteEntryId: "entry-unknown" } },
    ];
    const call = vi.fn(async (command: string) => {
      if (command === "browse_remote_playlist") return { provider: "spotify", playlistId: "remote-id", title: "Availability", tracks: rows, total: rows.length, complete: true };
      throw new Error(`Unexpected command ${command}`);
    });
    const target = render(call); await settle();
    const renderedRows = [...target.querySelectorAll<HTMLElement>(".shared-track-list li")];
    expect(renderedRows).toHaveLength(3);
    expect(renderedRows[0]?.textContent).toContain("Unavailable from Spotify");
    expect(renderedRows[1]?.textContent).not.toContain("Unavailable from Spotify");
    expect(renderedRows[2]?.textContent).not.toContain("Unavailable from Spotify");
    expect(renderedRows.map(row => row.textContent)).toEqual(expect.arrayContaining([
      expect.stringContaining("Playlist position 1"),
      expect.stringContaining("Playlist position 2"),
      expect.stringContaining("Playlist position 3"),
    ]));
  });

  it("restores loaded pages and playlist controls only for the same provider account revision", async () => {
    const url = "https://open.spotify.com/playlist/navigation-snapshot";
    const rows = [track("snapshot-a", "Blue song"), track("snapshot-b", "Red song")];
    const status = { accountRevisions: { spotify: 7 }, spotifyAccountId: "fixture-account" };
    const firstCall = vi.fn(async (command: string) => {
      if (command === "get_managed_provider_session_status") return status;
      if (command === "browse_remote_playlist") return { provider: "spotify", playlistId: "navigation-snapshot", title: "Snapshot mix", tracks: rows, nextCursor: null, total: 2, complete: true };
      throw new Error(`Unexpected command ${command}`);
    });
    const firstTarget = render(firstCall, { playlistUrl: url }); await settle();
    const filter = firstTarget.querySelector<HTMLInputElement>('[aria-label="Filter playlist tracks"]')!;
    filter.value = "Red"; filter.dispatchEvent(new Event("input", { bubbles: true })); await settle();
    const detail = firstTarget.querySelector<HTMLElement>(".remote-playlist-detail")!;
    detail.scrollTop = 73; detail.dispatchEvent(new Event("scroll"));
    unmountCurrent();

    const sameAccountCall = vi.fn(async (command: string) => {
      if (command === "get_managed_provider_session_status") return status;
      throw new Error(`Unexpected command ${command}`);
    });
    const restored = render(sameAccountCall, { playlistUrl: url }); await settle();
    expect(sameAccountCall).not.toHaveBeenCalledWith("browse_remote_playlist", expect.anything());
    expect(restored.querySelector<HTMLInputElement>('[aria-label="Filter playlist tracks"]')?.value).toBe("Red");
    expect(restored.querySelectorAll(".shared-track-list li")).toHaveLength(1);
    expect(restored.textContent).toContain("Red song");
    expect(restored.querySelector<HTMLElement>(".remote-playlist-detail")?.scrollTop).toBe(73);
    unmountCurrent();
    const switchedAccountCall = vi.fn(async (command: string) => {
      if (command === "get_managed_provider_session_status") return { accountRevisions: { spotify: 8 }, spotifyAccountId: "fixture-account" };
      if (command === "browse_remote_playlist") return { provider: "spotify", playlistId: "navigation-snapshot", title: "Snapshot mix", tracks: rows, nextCursor: null, total: 2, complete: true };
      throw new Error(`Unexpected command ${command}`);
    });
    const switched = render(switchedAccountCall, { playlistUrl: url }); await settle();
    expect(switchedAccountCall).toHaveBeenCalledWith("browse_remote_playlist", expect.objectContaining({ playlistId: url }));
    expect(switched.querySelector<HTMLInputElement>('[aria-label="Filter playlist tracks"]')?.value).toBe("Red");
  });

  it("stops deferred restored pages when the user edits track controls", async () => {
    const url = "https://open.spotify.com/playlist/deferred-restore";
    const status = { accountRevisions: { spotify: 20 }, spotifyAccountId: "fixture" };
    const originalCall = vi.fn(async (command: string, args?: Record<string, unknown>) => {
      if (command === "get_managed_provider_session_status") return status;
      if (command === "browse_remote_playlist") return args?.cursor
        ? { provider: "spotify", playlistId: "deferred-restore", tracks: [track("second", "Second")], total: 2, complete: true }
        : { provider: "spotify", playlistId: "deferred-restore", title: "Deferred", tracks: [track("first", "First")], nextCursor: "page-2", total: 2 };
      throw new Error(`Unexpected ${command}`);
    });
    const original = render(originalCall, { playlistUrl: url }); await settle();
    original.querySelector<HTMLButtonElement>(".remote-detail-more")!.click(); await settle();
    unmountCurrent();

    const delayed = deferred<unknown>();
    const restoringCall = vi.fn(async (command: string, args?: Record<string, unknown>) => {
      if (command === "get_managed_provider_session_status") return { ...status, accountRevisions: { spotify: 21 } };
      if (command === "browse_remote_playlist") return delayed.promise;
      throw new Error(`Unexpected ${command}`);
    });
    const restored = render(restoringCall, { playlistUrl: url }); await settle();
    const filter = restored.querySelector<HTMLInputElement>('[aria-label="Filter playlist tracks"]')!;
    filter.value = "my live filter"; filter.dispatchEvent(new Event("input", { bubbles: true })); await settle();
    delayed.resolve({ provider: "spotify", playlistId: "deferred-restore", tracks: [track("late", "Late restored")], nextCursor: "old-next", total: 2 }); await settle();
    expect(filter.value).toBe("my live filter");
    expect(restoringCall.mock.calls.filter(([command]) => command === "browse_remote_playlist")).toHaveLength(1);
    expect(restored.textContent).not.toContain("Late restored");
  });

  it("does not let an older account-scope response relabel the newer route snapshot", async () => {
    const firstScope = deferred<unknown>();
    let statusCalls = 0;
    const routeProps = proxy({
      provider: "spotify" as const,
      playlistUrl: "https://open.spotify.com/playlist/route-a",
      call: async (command: string): Promise<unknown> => {
        if (command === "get_managed_provider_session_status") {
          statusCalls += 1;
          return statusCalls === 1 ? firstScope.promise : { accountRevisions: { spotify: 2 }, spotifyAccountId: "account-b" };
        }
        if (command === "browse_remote_playlist") return { provider: "spotify", playlistId: "route-b", tracks: [track("route-b-track", "Route B")], total: 1, complete: true };
        throw new Error(`Unexpected command ${command}`);
      },
    });
    const target = document.createElement("div"); document.body.append(target);
    const instance = mount(RemotePlaylistDetail, { target, props: routeProps });
    destroy = () => unmount(instance);
    await settle();
    routeProps.playlistUrl = "https://open.spotify.com/playlist/route-b";
    await settle();
    firstScope.resolve({ accountRevisions: { spotify: 1 }, spotifyAccountId: "account-a" }); await settle();
    unmountCurrent();

    const restoredCall = vi.fn(async (command: string, args?: Record<string, unknown>) => {
      if (command === "get_managed_provider_session_status") return { accountRevisions: { spotify: 2 }, spotifyAccountId: "account-b" };
      throw new Error(`Unexpected command ${command}`);
    });
    const restored = render(restoredCall, { playlistUrl: "https://open.spotify.com/playlist/route-b" }); await settle();
    expect(restored.textContent).toContain("Route B");
    expect(restoredCall).not.toHaveBeenCalledWith("browse_remote_playlist", expect.anything());
  });

  it("filters tracks, loads more, and only imports on Add to library", async () => {
    const first = track("track-1", "Blue song"); const second = track("track-2", "Red song");
    const call = vi.fn(async (command: string, args?: Record<string, unknown>) => {
      if (command === "browse_remote_playlist") return args?.cursor
        ? { provider: "spotify", playlistId: "remote-id", tracks: [second], nextCursor: null, total: 2, complete: true }
        : { provider: "spotify", playlistId: "remote-id", title: "Remote mix", tracks: [first], nextCursor: "page-2", total: 2 };
      if (command === "import_remote_playlists") return { state: "completed", completed: 1, total: 1, results: [{ playlistId: "remote-id", collectionId: "local-id" }], errors: [] };
      throw new Error(`Unexpected command ${command}`);
    });
    const onImported = vi.fn(); const target = render(call, { onImported }); await settle();
    expect(target.querySelectorAll(".shared-track-list li")).toHaveLength(1);
    expect(target.textContent).toContain("1 of 2 tracks loaded");
    expect([...target.querySelectorAll<HTMLButtonElement>("button")].some(button => button.textContent?.trim() === "Retry loading playlist")).toBe(false);
    expect(target.querySelector(".remote-detail-more")?.textContent?.trim()).toBe("Load more tracks");
    expect([...target.querySelectorAll<HTMLButtonElement>(".remote-detail-actions button")].map(button=>button.textContent?.trim())).toEqual(["Play loaded tracks", "Queue loaded tracks", "Play loaded tracks next", "Add to library"]);
    expect(call).not.toHaveBeenCalledWith("import_remote_playlists", expect.anything());
    target.querySelector<HTMLButtonElement>(".remote-detail-more")!.click(); await settle();
    expect(call).toHaveBeenCalledWith("browse_remote_playlist", expect.objectContaining({ provider: "spotify", playlistId: "https://open.spotify.com/playlist/remote-id", cursor: "page-2", requestId: expect.any(String) }));
    expect(target.querySelectorAll(".shared-track-list li")).toHaveLength(2);
    expect(target.textContent).toContain("2 tracks");
    expect([...target.querySelectorAll<HTMLButtonElement>(".remote-detail-actions button")].slice(0,3).map(button=>button.textContent?.trim())).toEqual(["Play all tracks", "Queue all tracks", "Play all tracks next"]);
    const filter = target.querySelector<HTMLInputElement>('[aria-label="Filter playlist tracks"]')!;
    filter.value = "Red"; filter.dispatchEvent(new Event("input", { bubbles: true })); await settle();
    expect(target.querySelectorAll(".shared-track-list li")).toHaveLength(1);
    expect(target.textContent).toContain("Red song");
    expect(target.textContent).not.toContain("Blue song");
    expect(target.textContent).toContain("1 matching · 2 loaded of 2 tracks");
    expect([...target.querySelectorAll<HTMLButtonElement>(".remote-detail-actions button")].slice(0,3).map(button=>button.textContent?.trim())).toEqual(["Play matching tracks", "Queue matching tracks", "Play matching tracks next"]);
    [...target.querySelectorAll<HTMLButtonElement>("button")].find(button => button.textContent === "Add to library")!.click(); await settle();
    expect(call).toHaveBeenCalledWith("import_remote_playlists", { provider: "spotify", playlistIds: ["https://open.spotify.com/playlist/remote-id"] });
    expect(onImported).toHaveBeenCalledTimes(1);
  });

  it("does not report an empty failed import as added and labels incomplete imported playlists", async () => {
    const failedCall = vi.fn(async (command: string) => {
      if (command === "browse_remote_playlist") return { provider: "spotify", playlistId: "remote-id", title: "Remote mix", tracks: [], complete: true };
      if (command === "import_remote_playlists") return { state: "partial", completed: 1, total: 1, results: [], errors: [{ playlistId: "remote-id", message: "Provider refused the playlist" }] };
      throw new Error(`Unexpected command ${command}`);
    });
    const failedImported = vi.fn();
    const failed = render(failedCall, { onImported: failedImported }); await settle();
    [...failed.querySelectorAll<HTMLButtonElement>("button")].find(button => button.textContent?.trim() === "Add to library")!.click(); await settle();
    expect(failed.querySelector('[role="alert"]')?.textContent).toContain("Provider refused");
    expect(failed.textContent).not.toContain("Playlist added to your library.");
    expect(failedImported).not.toHaveBeenCalled();

    unmountCurrent();
    const incompleteCall = vi.fn(async (command: string) => {
      if (command === "browse_remote_playlist") return { provider: "spotify", playlistId: "remote-id", title: "Remote mix", tracks: [], complete: true };
      if (command === "import_remote_playlists") return { state: "partial", completed: 1, total: 1, results: [{ playlistId: "remote-id", collectionId: "local-id", complete: false }], errors: [] };
      throw new Error(`Unexpected command ${command}`);
    });
    const incompleteImported = vi.fn();
    const incomplete = render(incompleteCall, { onImported: incompleteImported }); await settle();
    [...incomplete.querySelectorAll<HTMLButtonElement>("button")].find(button => button.textContent?.trim() === "Add to library")!.click(); await settle();
    expect(incomplete.querySelector('[role="status"]')?.textContent).toContain("missing items");
    expect(incompleteImported).toHaveBeenCalledTimes(1);
  });

  it("reports a completed detail import as added when cancellation races with its terminal status", async () => {
    const call = vi.fn(async (command: string) => {
      if (command === "browse_remote_playlist") return { provider: "spotify", playlistId: "remote-id", title: "Race playlist", tracks: [], complete: true };
      if (command === "import_remote_playlists") return { jobId: "race-detail", state: "running", completed: 0, total: 1 };
      if (command === "cancel_remote_playlist_import") return undefined;
      if (command === "get_remote_playlist_import_job") return { jobId: "race-detail", state: "completed", completed: 1, total: 1, results: [{ playlistId: "remote-id", collectionId: "local-id", complete: true }], errors: [] };
      throw new Error(`Unexpected command ${command}`);
    });
    const onImported = vi.fn(); const target = render(call, { onImported }); await settle();
    [...target.querySelectorAll<HTMLButtonElement>("button")].find(button => button.textContent?.trim() === "Add to library")!.click(); await settle();
    [...target.querySelectorAll<HTMLButtonElement>("button")].find(button => button.textContent?.trim() === "Cancel import")!.click(); await settle();
    await new Promise(resolve => setTimeout(resolve, 850)); await settle();
    expect(call).toHaveBeenCalledWith("cancel_remote_playlist_import", { jobId: "race-detail" });
    expect(target.querySelector('[role="status"]')?.textContent).toContain("Playlist added to your library.");
    expect(target.textContent).not.toContain("Import canceled.");
    expect(onImported).toHaveBeenCalledTimes(1);
  });

  it("reports a partial detail import when cancellation races with its terminal status", async () => {
    const call = vi.fn(async (command: string) => {
      if (command === "browse_remote_playlist") return { provider: "spotify", playlistId: "remote-id", title: "Partial race playlist", tracks: [], complete: true };
      if (command === "import_remote_playlists") return { jobId: "partial-race-detail", state: "running", completed: 0, total: 1 };
      if (command === "cancel_remote_playlist_import") return undefined;
      if (command === "get_remote_playlist_import_job") return { jobId: "partial-race-detail", state: "partial", completed: 1, total: 1, results: [{ playlistId: "remote-id", collectionId: "local-id", complete: false }], errors: [] };
      throw new Error(`Unexpected command ${command}`);
    });
    const onImported = vi.fn(); const target = render(call, { onImported }); await settle();
    [...target.querySelectorAll<HTMLButtonElement>("button")].find(button => button.textContent?.trim() === "Add to library")!.click(); await settle();
    [...target.querySelectorAll<HTMLButtonElement>("button")].find(button => button.textContent?.trim() === "Cancel import")!.click(); await settle();
    await new Promise(resolve => setTimeout(resolve, 850)); await settle();
    expect(target.querySelector('[role="status"]')?.textContent).toContain("Playlist added with missing items.");
    expect(target.textContent).not.toContain("Import canceled.");
    expect(onImported).toHaveBeenCalledTimes(1);
  });

  it("uses the terminal continuation flag with the cumulative global track total", async () => {
    const firstPage = Array.from({ length: 230 }, (_, index) => track(`global-${index + 1}`, `Playlist track ${index + 1}`));
    const finalPage = Array.from({ length: 10 }, (_, index) => track(`global-${index + 231}`, `Playlist track ${index + 231}`));
    const call = vi.fn(async (command: string, args?: Record<string, unknown>) => {
      if (command !== "browse_remote_playlist") throw new Error(`Unexpected command ${command}`);
      return args?.cursor
        ? { provider: "spotify", playlistId: "remote-id", tracks: finalPage, nextCursor: null, total: 240, complete: true }
        : { provider: "spotify", playlistId: "remote-id", title: "Large playlist", tracks: firstPage, nextCursor: "page-2", total: 240, complete: false };
    });
    const onQueueTracks = vi.fn();
    const target = render(call, { onQueueTracks }); await settle();
    expect(target.textContent).toContain("230 of 240 tracks loaded");
    target.querySelector<HTMLButtonElement>(".remote-detail-more")!.click(); await settle();
    expect(target.textContent).toContain("240 tracks");
    expect(target.textContent).not.toContain("Snapshot incomplete");
    expect(target.querySelector(".remote-detail-retry")).toBeNull();
    [...target.querySelectorAll<HTMLButtonElement>(".remote-detail-actions button")].find(button => button.textContent?.startsWith("Queue "))!.click();
    expect(onQueueTracks).toHaveBeenCalledWith([...firstPage, ...finalPage], "append");
  });

  it("preserves repeated playlist occurrences and deduplicates only a page overlap", async () => {
    const firstOccurrence = { ...track("repeat", "Repeated song"), provenance: { remoteEntryId: "entry-1", position: 1 } };
    const middle = { ...track("middle", "Middle song"), provenance: { remoteEntryId: "entry-2", position: 2 } };
    const secondOccurrence = { ...track("repeat", "Repeated song"), provenance: { remoteEntryId: "entry-3", position: 3 } };
    const overlappingSecond = { ...secondOccurrence, provenance: { remoteEntryId: "entry-3", position: 1 } };
    const last = { ...track("last", "Last song"), provenance: { remoteEntryId: "entry-4", position: 4 } };
    const call = vi.fn(async (command: string, args?: Record<string, unknown>) => {
      if (command === "browse_remote_playlist") return args?.cursor
        ? { provider: "spotify", playlistId: "remote-id", tracks: [overlappingSecond, last], nextCursor: null, total: 4, complete: true }
        : { provider: "spotify", playlistId: "remote-id", title: "Remote mix", tracks: [firstOccurrence, middle, secondOccurrence], nextCursor: "page-2", total: 4 };
      throw new Error(`Unexpected command ${command}`);
    });
    const onQueueTracks = vi.fn();
    const target = render(call, { onQueueTracks }); await settle();
    target.querySelector<HTMLButtonElement>(".remote-detail-more")!.click(); await settle();
    expect(target.querySelectorAll(".shared-track-list li")).toHaveLength(4);
    [...target.querySelectorAll<HTMLButtonElement>("button")].find(button => button.textContent === "Queue all tracks")!.click();
    expect(onQueueTracks).toHaveBeenCalledWith([firstOccurrence, middle, secondOccurrence, last], "append");
  });

  it("keeps first-page playlist metadata across a misleading continuation and refreshes it on retry", async () => {
    const first = track("first", "First playlist track");
    const continuation = track("next", "Continuation track");
    const refreshed = track("fresh", "Fresh playlist track");
    let browseCount = 0;
    const call = vi.fn(async (command: string, args?: Record<string, unknown>) => {
      if (command === "get_managed_provider_session_status") return { accountRevisions: { spotify: 4 }, spotifyAccountId: "fixture" };
      if (command !== "browse_remote_playlist") throw new Error(`Unexpected command ${command}`);
      browseCount += 1;
      if (browseCount === 1) return {
        provider: "spotify", playlistId: "remote-id", title: "Trusted playlist", url: "https://open.spotify.com/playlist/remote-id",
        ownerName: "Trusted owner", ownerUrl: "https://open.spotify.com/user/trusted-owner", artworkUrl: "https://images.example/trusted.jpg",
        visibility: "Private", canEdit: false, tracks: [first], nextCursor: "continuation", total: 240, complete: false,
      };
      if (browseCount === 2) return {
        provider: "spotify", playlistId: "recommendation-id", title: "Recommended sidebar row", url: "https://open.spotify.com/playlist/recommendation-id",
        ownerName: "Different owner", ownerUrl: "https://open.spotify.com/user/different-owner", artworkUrl: "https://images.example/wrong.jpg",
        visibility: "Public", canEdit: true, tracks: [continuation], nextCursor: null, total: 5, complete: true,
      };
      return {
        provider: "spotify", playlistId: "remote-id", title: "Refreshed playlist", ownerName: "Refreshed owner",
        artworkUrl: "https://images.example/refreshed.jpg", visibility: "Public", canEdit: true,
        tracks: [refreshed], nextCursor: null, total: 1, complete: true,
      };
    });
    const target = render(call); await settle();
    target.querySelector<HTMLButtonElement>(".remote-detail-more")!.click(); await settle();

    expect(target.querySelector("#remote-playlist-detail-title")?.textContent).toBe("Trusted playlist");
    expect(target.textContent).toContain("Trusted owner");
    expect(target.textContent).toContain("Private");
    expect(target.textContent).toContain("2 of 240 tracks loaded");
    expect(target.textContent).toContain("Snapshot incomplete");
    expect(target.querySelector(".remote-detail-retry")).not.toBeNull();
    expect(target.querySelector(".remote-detail-actions .primary")?.textContent).toContain("Add to library");
    expect(target.textContent).toContain("Continuation track");

    unmountCurrent();
    const saved = providerNavigationSnapshots.get<{ page: Record<string, unknown> }>(
      "playlist-detail", "spotify:https://open.spotify.com/playlist/remote-id", "spotify:revision:4:fixture",
    );
    expect(saved?.page).toMatchObject({
      provider: "spotify", playlistId: "remote-id", title: "Trusted playlist", ownerName: "Trusted owner",
      ownerUrl: "https://open.spotify.com/user/trusted-owner", artworkUrl: "https://images.example/trusted.jpg",
      visibility: "Private", canEdit: false, total: 240, nextCursor: null, complete: false,
    });

    const refreshedTarget = render(call); await settle();
    refreshedTarget.querySelector<HTMLButtonElement>(".remote-detail-retry")!.click(); await settle();
    expect(refreshedTarget.querySelector("#remote-playlist-detail-title")?.textContent).toBe("Refreshed playlist");
    expect(refreshedTarget.textContent).toContain("Refreshed owner");
    expect(refreshedTarget.textContent).toContain("1 track");
    expect(refreshedTarget.textContent).not.toContain("Trusted owner");
    expect(refreshedTarget.textContent).not.toContain("Snapshot incomplete");
    expect(refreshedTarget.textContent).toContain("Fresh playlist track");
  });

  it("keeps an overshooting append incomplete when loaded rows exceed the known total", async () => {
    const call = vi.fn(async (command: string, args?: Record<string, unknown>) => {
      if (command === "get_managed_provider_session_status") return { accountRevisions: { spotify: 4 }, spotifyAccountId: "fixture" };
      if (command !== "browse_remote_playlist") throw new Error(`Unexpected command ${command}`);
      return args?.cursor
        ? { provider: "spotify", playlistId: "remote-id", tracks: [track("second"), track("third")], nextCursor: null, total: 2, complete: true }
        : { provider: "spotify", playlistId: "remote-id", title: "Two-track playlist", tracks: [track("first")], nextCursor: "page-2", total: 2, complete: false };
    });
    const target = render(call); await settle();
    target.querySelector<HTMLButtonElement>(".remote-detail-more")!.click(); await settle();
    expect(target.textContent).toContain("Snapshot incomplete");
    expect(target.querySelector(".remote-detail-retry")).not.toBeNull();
    expect(target.querySelectorAll(".shared-track-list li")).toHaveLength(3);
  });

  it("stops a repeated cursor while retaining the loaded rows and reporting an incomplete playlist", async () => {
    const first = track("first", "First song");
    const second = track("second", "Second song");
    const call = vi.fn(async (command: string, args?: Record<string, unknown>) => {
      if (command === "browse_remote_playlist") return args?.cursor
        ? { provider: "spotify", playlistId: "remote-id", tracks: [second], nextCursor: "page-2", total: 3, complete: true }
        : { provider: "spotify", playlistId: "remote-id", tracks: [first], nextCursor: "page-2", total: 3 };
      throw new Error(`Unexpected command ${command}`);
    });
    const target = render(call); await settle();
    target.querySelector<HTMLButtonElement>(".remote-detail-more")!.click(); await settle();
    expect(target.querySelectorAll(".shared-track-list li")).toHaveLength(2);
    expect(target.textContent).toContain("Stopped loading because the provider repeated a page cursor");
    expect(target.textContent).toContain("Snapshot incomplete");
    expect(target.querySelector(".remote-detail-more")).toBeNull();
    expect([...target.querySelectorAll<HTMLButtonElement>(".remote-detail-actions button")].slice(0, 3).map(button => button.textContent?.trim())).toEqual([
      "Play loaded tracks", "Queue loaded tracks", "Play loaded tracks next",
    ]);
  });

  it("retries an incomplete cursorless snapshot from the first page and replaces partial rows", async () => {
    const partial = track("partial", "Partial page song");
    const refreshed = track("fresh", "Fresh first-page song");
    const failedRetry = deferred<unknown>();
    const successfulRetry = deferred<unknown>();
    let browseCount = 0;
    const call = vi.fn(async (command: string, args?: Record<string, unknown>) => {
      if (command !== "browse_remote_playlist") throw new Error(`Unexpected command ${command}`);
      browseCount += 1;
      if (browseCount === 1) return { provider: "spotify", playlistId: "remote-id", title: "Incomplete mix", tracks: [partial], nextCursor: null, total: null, complete: false };
      return browseCount === 2 ? failedRetry.promise : successfulRetry.promise;
    });
    const target = render(call); await settle();
    expect(target.textContent).toContain("Snapshot incomplete");
    expect(target.querySelector(".remote-detail-more")).toBeNull();
    const retryButton = [...target.querySelectorAll<HTMLButtonElement>("button")].find(button => button.textContent?.trim() === "Retry loading playlist");
    expect(retryButton).toBeDefined();

    retryButton!.click(); await settle();
    expect(call.mock.calls.filter(([command]) => command === "browse_remote_playlist")).toHaveLength(2);
    expect(call.mock.calls[1]?.[1]).not.toHaveProperty("cursor");
    expect(target.textContent).toContain("Snapshot incomplete");
    expect(target.textContent).toContain("Retrying playlist…");
    expect(target.textContent).toContain("Partial page song");
    expect([...target.querySelectorAll<HTMLButtonElement>("button")].some(button => button.textContent?.trim() === "Retrying playlist…" && button.disabled)).toBe(true);

    failedRetry.reject(new Error("temporary provider error")); await settle();
    expect(target.textContent).toContain("Snapshot incomplete");
    expect(target.textContent).toContain("Partial page song");
    expect(target.textContent).toContain("Could not load playlist tracks");
    const retryAgain = [...target.querySelectorAll<HTMLButtonElement>("button")].find(button => button.textContent?.trim() === "Retry loading playlist");
    expect(retryAgain).toBeDefined();
    retryAgain!.click(); await settle();
    expect(call.mock.calls.filter(([command]) => command === "browse_remote_playlist")).toHaveLength(3);
    expect(call.mock.calls[2]?.[1]).not.toHaveProperty("cursor");
    expect(target.textContent).toContain("Partial page song");
    successfulRetry.resolve({ provider: "spotify", playlistId: "remote-id", title: "Fresh mix", tracks: [refreshed], nextCursor: null, total: 1, complete: true });
    await settle();
    expect(target.textContent).toContain("Fresh first-page song");
    expect(target.textContent).not.toContain("Partial page song");
    expect(target.textContent).not.toContain("Snapshot incomplete");
    expect([...target.querySelectorAll<HTMLButtonElement>("button")].some(button => button.textContent?.trim() === "Retry loading playlist")).toBe(false);
  });

  it("ignores stale browse results and resets playlist state when identity changes", async () => {
    let resolveOld!: (value: unknown) => void;
    const oldResult = new Promise<unknown>(resolve => { resolveOld = resolve; });
    const call = vi.fn(async (command: string, args?: Record<string, unknown>) => {
      if (command === "browse_remote_playlist" && args?.provider === "spotify") return oldResult;
      if (command === "browse_remote_playlist") return { provider: "youtube", playlistId: "new-id", title: "New playlist", tracks: [track("new", "New song")], total: 1, complete: true };
      return {};
    });
    const target = document.createElement("div"); document.body.append(target);
    const props = proxy<{ provider: "spotify" | "youtube"; playlistUrl: string; call: typeof call }>({ provider: "spotify", playlistUrl: "old-url", call });
    const instance = mount(RemotePlaylistDetail, { target, props });
    destroy = () => unmount(instance);
    await settle();
    props.provider = "youtube";
    props.playlistUrl = "new-url";
    await settle();
    resolveOld({ provider: "spotify", playlistId: "old-id", title: "Old playlist", tracks: [track("old", "Old song")], total: 1, complete: true });
    await settle();
    expect(target.textContent).toContain("New playlist");
    expect(target.textContent).toContain("New song");
    expect(target.textContent).not.toContain("Old playlist");
    expect(target.textContent).not.toContain("Old song");
    expect(call).toHaveBeenCalledWith("browse_remote_playlist", expect.objectContaining({ provider: "youtube", playlistId: "new-url", requestId: expect.any(String) }));
  });

  it("filters by duration, sorts the displayed tracks, and matches full playback identity", async () => {
    const rows = [
      { ...track("short", "Short song"), durationSeconds: 60 },
      { ...track("long", "Long song"), durationSeconds: 120 },
    ];
    const call = vi.fn(async (command: string) => {
      if (command === "browse_remote_playlist") return { provider: "spotify", playlistId: "remote-id", title: "Remote mix", tracks: rows, total: 2 };
      throw new Error(`Unexpected command ${command}`);
    });
    const target = render(call, { playingTrack: { ...rows[1], title: "Refreshed title" }, bufferingTrack: rows[1], playingTrackId: "stale-id", bufferingTrackId: "stale-id" }); await settle();
    const sort = target.querySelector<HTMLSelectElement>('[aria-label="Sort playlist tracks"]')!;
    sort.value = "title"; sort.dispatchEvent(new Event("change", { bubbles: true })); await settle();
    expect([...target.querySelectorAll<HTMLButtonElement>(".track-title")].map(button => button.textContent?.trim())).toEqual(["Long song", "Short song"]);
    const direction = target.querySelector<HTMLSelectElement>('[aria-label="Sort direction"]')!;
    direction.value = "desc"; direction.dispatchEvent(new Event("change", { bubbles: true })); await settle();
    expect([...target.querySelectorAll<HTMLButtonElement>(".track-title")].map(button => button.textContent?.trim())).toEqual(["Short song", "Long song"]);
    expect(target.querySelectorAll(".current")).toHaveLength(1);
    expect(target.querySelectorAll(".buffering")).toHaveLength(1);

    const filters = target.querySelector<HTMLButtonElement>(".filter-toggle")!;
    expect(filters.getAttribute("aria-expanded")).toBe("false");
    filters.click(); await settle();
    const min = document.body.querySelector<HTMLInputElement>('[aria-label="Minimum track duration"]')!;
    min.value = "1:30"; min.dispatchEvent(new Event("input", { bubbles: true })); await settle();
    expect(target.querySelectorAll(".shared-track-list li")).toHaveLength(1);
    expect(target.querySelector(".track-title")?.textContent).toContain("Long song");
    filters.click(); await settle();
    expect(min.value).toBe("1:30");
    expect(filters.getAttribute("aria-expanded")).toBe("false");
  });

  it("keeps sort fields available when filters hide values from the complete playlist", async () => {
    const rows = [
      { ...track("known-bpm", "Known tempo"), durationSeconds: 60, bpm: 124 },
      { ...track("unknown-bpm", "Unknown tempo"), durationSeconds: 120 },
    ];
    const call = vi.fn(async (command: string) => {
      if (command === "browse_remote_playlist") return { provider: "spotify", playlistId: "remote-id", title: "Filtered availability", tracks: rows, total: 2, complete: true };
      throw new Error(`Unexpected command ${command}`);
    });
    const target = render(call); await settle();
    const bpmOption = () => target.querySelector<HTMLSelectElement>('[aria-label="Sort playlist tracks"]')?.querySelector<HTMLOptionElement>('option[value="bpm"]');
    expect(bpmOption()?.disabled).toBe(false);

    const minimum = target.querySelector<HTMLInputElement>('[aria-label="Minimum track duration"]')!;
    minimum.value = "1:30"; minimum.dispatchEvent(new Event("input", { bubbles: true })); await settle();
    expect([...target.querySelectorAll<HTMLButtonElement>(".track-title")].map(button => button.textContent?.trim())).toEqual(["Unknown tempo"]);
    expect(bpmOption()?.disabled).toBe(false);
    expect(bpmOption()?.textContent).toBe("BPM");

    minimum.value = ""; minimum.dispatchEvent(new Event("input", { bubbles: true })); await settle();
    expect(bpmOption()?.disabled).toBe(false);
  });

  it("adds selected repeated playlist occurrences after sorting", async () => {
    const repeated = { ...track("repeat", "Zulu"), provenance: { remoteEntryId: "remote-entry-1", position: 1 } };
    const other = { ...track("other", "Alpha"), provenance: { remoteEntryId: "remote-entry-2", position: 2 } };
    const repeatedAgain = { ...repeated, provenance: { remoteEntryId: "remote-entry-3", position: 3 } };
    const call = vi.fn(async (command: string) => {
      if (command === "browse_remote_playlist") return { provider: "spotify", playlistId: "remote-id", title: "Remote mix", tracks: [repeated, other, repeatedAgain], complete: true };
      throw new Error(`Unexpected command ${command}`);
    });
    const onAddSelectedToPlaylist = vi.fn();
    const target = render(call, { onAddSelectedToPlaylist }); await settle();
    const sort = target.querySelector<HTMLSelectElement>('[aria-label="Sort playlist tracks"]')!;
    sort.value = "title"; sort.dispatchEvent(new Event("change", { bubbles: true })); await settle();
    target.querySelector<HTMLButtonElement>('.start-selection')!.click(); await settle();
    const checks = [...target.querySelectorAll<HTMLInputElement>('[aria-label="Select Zulu for playlist"]')];
    checks[0]!.click(); checks[1]!.click();
    await settle();
    sort.value = "sourceOrder"; sort.dispatchEvent(new Event("change", { bubbles: true })); await settle();
    target.querySelector<HTMLButtonElement>(".add-selected")!.click();
    expect(onAddSelectedToPlaylist).toHaveBeenCalledWith([repeated, repeatedAgain]);
  });

  it("keeps action scope limited to loaded tracks when completeness is unknown", async () => {
    const call = vi.fn(async (command: string) => {
      if (command === "browse_remote_playlist") return { provider: "spotify", playlistId: "remote-id", title: "Uncertain mix", tracks: [track("loaded")], total: null };
      throw new Error(`Unexpected command ${command}`);
    });
    const target = render(call); await settle();
    expect(target.textContent).toContain("1 track loaded");
    expect([...target.querySelectorAll<HTMLButtonElement>(".remote-detail-actions button")].slice(0, 3).map(button => button.textContent?.trim())).toEqual([
      "Play loaded tracks", "Queue loaded tracks", "Play loaded tracks next",
    ]);
  });

  it("does not call a cursorless playlist complete when completeness is false or unknown", async () => {
    const makeCall = (complete?: boolean) => vi.fn(async (command: string) => {
      if (command === "browse_remote_playlist") return { provider: "spotify", playlistId: "remote-id", title: "Uncertain total", tracks: [track("one")], total: 1, complete };
      throw new Error(`Unexpected command ${command}`);
    });
    const incomplete = render(makeCall(false)); await settle();
    expect(incomplete.textContent).toContain("1 of 1 track loaded");
    expect(incomplete.textContent).toContain("Snapshot incomplete");
    expect(incomplete.querySelector<HTMLSelectElement>('[aria-label="Sort playlist tracks"]')?.querySelector<HTMLOptionElement>('option[value="bpm"]')?.disabled).toBe(false);
    expect(incomplete.textContent).not.toMatch(/(?<!of )1 tracks?(?! loaded)/);
    destroy?.(); destroy = undefined;

    const unknown = render(makeCall(undefined)); await settle();
    expect(unknown.textContent).toContain("1 of 1 track loaded");
    expect(unknown.textContent).toContain("Completeness unknown");
    expect(unknown.querySelector<HTMLSelectElement>('[aria-label="Sort playlist tracks"]')?.querySelector<HTMLOptionElement>('option[value="bpm"]')?.disabled).toBe(false);
    expect(unknown.textContent).not.toMatch(/(?<!of )1 tracks?(?! loaded)/);
  });

  it("wraps the playlist heading at narrow widths", async () => {
    const title = "Salsa Greatest Hits | Songs for the full dance floor and the late set";
    const call = vi.fn(async (command: string) => {
      if (command === "browse_remote_playlist") return { provider: "spotify", playlistId: "remote-id", title, tracks: [], total: 0 };
      throw new Error(`Unexpected command ${command}`);
    });
    const target = render(call); await settle();
    const heading = target.querySelector<HTMLHeadingElement>("#remote-playlist-detail-title")!;
    expect(heading.textContent).toBe(title);
  });
});
