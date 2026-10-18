// @vitest-environment jsdom
// @ts-expect-error Exercise Svelte's browser runtime in jsdom.
import { mount, tick, unmount } from "../node_modules/svelte/src/index-client.js";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
// @ts-expect-error Match the browser runtime used by the direct import above.
vi.mock("svelte", async () => await import("../node_modules/svelte/src/index-client.js"));
import RemotePlaylists from "./RemotePlaylists.svelte";
import { providerNavigationSnapshots } from "./lib/provider-navigation-snapshots";
import { appendPageWithinLimit } from "./remote-playlist-paging";

let destroy: (() => void) | undefined;
beforeEach(() => {
  providerNavigationSnapshots.clear();
  vi.stubGlobal("requestAnimationFrame", (callback: FrameRequestCallback) => { callback(0); return 1; });
});
afterEach(() => { vi.useRealTimers(); vi.unstubAllGlobals(); destroy?.(); destroy = undefined; document.body.innerHTML = ""; });
async function settle() { for (let i = 0; i < 10; i++) await Promise.resolve(); await tick(); }
function deferred<T = unknown>() {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>(done => { resolve = done; });
  return { promise, resolve };
}
function render(call: (command: string, args?: Record<string, unknown>) => Promise<unknown>, props: { onClose?: () => void; initialOwner?: string; initialPlaylistUrl?: string; initialProvider?: "spotify" | "youtube"; onBrowseOwner?: (provider: "spotify" | "youtube", ownerUrl: string) => void } = {}) {
  const target = document.createElement("div"); document.body.append(target);
  const instance = mount(RemotePlaylists, { target, props: { call, onImported: vi.fn(), initialProvider: "spotify", ...props } });
  destroy = () => unmount(instance);
  return target;
}

describe("RemotePlaylists", () => {
  it("keeps the close action in the provider row with accessible labels and callback", async () => {
    const onClose = vi.fn();
    const call = vi.fn(async (command: string) => {
      if (command === "get_managed_provider_session_status") return { spotifyAccountId: "fixture" };
      if (command === "list_remote_playlist_bindings") return [];
      if (command === "list_remote_playlists") return { items: [] };
      throw new Error(`Unexpected ${command}`);
    });
    const target = render(call, { onClose }); await settle();
    const row = target.querySelector('[aria-label="Playlist provider"]')!;
    expect(row.querySelectorAll("button")).toHaveLength(3);
    expect(target.querySelector(".remote-playlists-header")).toBeNull();
    const close = row.querySelector<HTMLButtonElement>('[aria-label="Close remote playlist picker"]')!;
    expect(close).not.toBeNull();
    close.click();
    expect(onClose).toHaveBeenCalledTimes(1);
    [...row.querySelectorAll<HTMLButtonElement>("button")].find(button => button.textContent === "YouTube")!.click(); await settle();
    expect(target.querySelector(".remote-playlists")?.getAttribute("aria-label")).toBe("YouTube playlists");
    expect([...row.querySelectorAll("button")].find(button => button.textContent === "YouTube")?.getAttribute("aria-pressed")).toBe("true");
  });

  it("uses provider-specific accessible region labels without a redundant Playlists heading", async () => {
    const call = vi.fn(async (command: string) => {
      if (command === "get_managed_provider_session_status") return { spotifyAccountId: "fixture" };
      if (command === "list_remote_playlist_bindings") return [];
      if (command === "list_remote_playlists") return { items: [], nextCursor: null };
      throw new Error(`Unexpected ${command}`);
    });
    const target = render(call); await settle();
    expect(target.querySelector(".remote-playlists")?.getAttribute("aria-label")).toBe("Spotify playlists");
    expect([...target.querySelectorAll("h1,h2,h3")].some(node => node.textContent?.trim() === "Playlists")).toBe(false);
    expect(target.querySelector('[aria-label="Close remote playlist picker"]')).not.toBeNull();
    [...target.querySelectorAll<HTMLButtonElement>(".remote-provider-tabs button")].find(button => button.textContent === "YouTube")!.click(); await settle();
    expect(target.querySelector(".remote-playlists")?.getAttribute("aria-label")).toBe("YouTube playlists");
  });

  it("reports unique displayed progress for overlapping incomplete profile pages", async () => {
    const call = vi.fn(async (command: string, args?: Record<string, unknown>) => {
      if (command === "get_managed_provider_session_status") return { spotifyAccountId: "fixture" };
      if (command === "list_remote_playlist_bindings") return [];
      if (command === "list_remote_playlists") return args?.cursor
        ? { items: [{ id: "same", title: "Same" }, { id: "new", title: "New" }], nextCursor: null, totalCount: 110, truncated: true, complete: false, snapshotConsistency: "progressive" }
        : { items: [{ id: "same", title: "Same" }], nextCursor: "page2", totalCount: 110, truncated: true, complete: false, snapshotConsistency: "progressive" };
      throw new Error(`Unexpected ${command}`);
    });
    const target = render(call, { initialOwner: "profile" }); await settle();
    expect(target.textContent).toContain("Showing 1 loaded playlists.");
    target.querySelector<HTMLButtonElement>(".remote-more")!.click(); await settle();
    expect(target.querySelectorAll(".remote-result-row")).toHaveLength(2);
    expect(target.textContent).toContain("Showing 2 loaded playlists. The provider returned a bounded or incomplete result; it may omit playlists. 110 total items were reported. Playlists load progressively; refresh to reflect changes.");
    expect(target.textContent).not.toContain("Showing 3 loaded");
    expect(target.querySelector(".remote-more")).toBeNull();
  });

  it("clears the partial-profile warning when Load more reaches a confirmed complete result", async () => {
    const call=vi.fn(async(command:string,args?:Record<string,unknown>)=>{
      if(command==="list_remote_playlist_bindings")return [];
      if(command==="list_remote_playlists")return args?.cursor
        ? {items:[{id:"second",title:"Second"}],nextCursor:null,totalCount:2,truncated:false,complete:true}
        : {items:[{id:"first",title:"First"}],nextCursor:"prefix-cursor",totalCount:2,truncated:true,complete:false};
      throw Error(`Unexpected ${command}`);
    });
    const target=render(call,{initialOwner:"profile"});await settle();
    expect(target.textContent).toContain("bounded or incomplete");
    target.querySelector<HTMLButtonElement>(".remote-more")!.click();await settle();
    expect(target.textContent).not.toContain("bounded or incomplete");
    expect(target.querySelector(".remote-more")).toBeNull();
    target.querySelector<HTMLButtonElement>(".remote-selection-actions button")!.click();await settle();
    expect(target.textContent).toContain("Selected all 2 playlists in this browse.");
  });
  it("keeps the progressive consistency notice after appending a page", async () => {
    const status = { accountRevisions: { spotify: 6 }, spotifyAccountId: "fixture" };
    const call = vi.fn(async (command: string, args?: Record<string, unknown>) => {
      if (command === "get_managed_provider_session_status") return status;
      if (command === "list_remote_playlist_bindings") return [];
      if (command === "list_remote_playlists") return args?.cursor
        ? { items: [{ id: "second", title: "Second" }], nextCursor: "page-3", complete: false, snapshotConsistency: "progressive" }
        : { items: [{ id: "first", title: "First" }], nextCursor: "page-2", snapshotConsistency: "progressive" };
      throw new Error(`Unexpected ${command}`);
    });
    const target = render(call, { initialOwner: "profile" }); await settle();
    expect(target.textContent).toContain("Playlists load progressively; refresh to reflect changes.");
    target.querySelector<HTMLButtonElement>(".remote-more")!.click(); await settle();
    expect(target.textContent).toContain("Playlists load progressively; refresh to reflect changes.");
    expect(target.querySelectorAll(".remote-result-row")).toHaveLength(2);
    expect(target.textContent).toContain("2 playlists loaded. Load more to continue. Playlists load progressively; refresh to reflect changes.");
    expect(target.querySelectorAll(".remote-notice")).toHaveLength(1);
    destroy?.(); destroy = undefined;
    const restored = render(call, { initialOwner: "profile" }); await settle();
    expect(restored.textContent).toContain("Playlists load progressively; refresh to reflect changes.");
    expect(restored.querySelectorAll(".remote-result-row")).toHaveLength(2);
  });

  it("reports ordinary progressive paging without calling it truncated", async () => {
    const call = vi.fn(async (command: string, args?: Record<string, unknown>) => {
      if (command === "list_remote_playlist_bindings") return [];
      if (command !== "list_remote_playlists") throw new Error(`Unexpected command ${command}`);
      const page = args?.cursor;
      if (!page) return { items: [{ id: "first", title: "First" }], nextCursor: "page-2", totalCount: 3, complete: false, snapshotConsistency: "progressive" };
      if (page === "page-2") return { items: [{ id: "second", title: "Second" }], nextCursor: "page-3", totalCount: 3, complete: false, snapshotConsistency: "progressive" };
      return { items: [{ id: "third", title: "Third" }], nextCursor: null, totalCount: 3, complete: true, snapshotConsistency: "progressive" };
    });
    const target = render(call, { initialOwner: "profile" }); await settle();
    expect(target.textContent).toContain("Showing 1 of 3 playlists. Load more to continue.");
    expect(target.textContent).toContain("Playlists load progressively; refresh to reflect changes.");
    expect(target.textContent).not.toContain("bounded or incomplete");
    expect(target.querySelectorAll(".remote-notice")).toHaveLength(1);
    target.querySelector<HTMLButtonElement>(".remote-more")!.click(); await settle();
    expect(target.textContent).toContain("Showing 2 of 3 playlists. Load more to continue.");
    expect(target.querySelectorAll(".remote-notice")).toHaveLength(1);
    target.querySelector<HTMLButtonElement>(".remote-more")!.click(); await settle();
    expect(target.querySelectorAll(".remote-result-row")).toHaveLength(3);
    expect(target.textContent).not.toContain("bounded or incomplete");
    expect(target.querySelectorAll(".remote-notice")).toHaveLength(1);
    expect(target.textContent).toContain("Playlists load progressively; refresh to reflect changes.");
  });
  it("keeps user query/provider edits made while account scope is pending", async () => {
    const account = deferred<unknown>();
    const call = vi.fn(async (command: string, args?: Record<string, unknown>) => {
      if (command === "get_managed_provider_session_status") return account.promise;
      if (command === "list_remote_playlist_bindings") return [];
      if (command === "search_remote_playlists") return { items: [{ id: "fresh", title: "Fresh query result" }] };
      if (command === "list_remote_playlists") return { items: [] };
      throw new Error(`Unexpected command ${command}`);
    });
    const target = render(call); await settle();
    target.querySelector<HTMLButtonElement>(".remote-source-tabs button:nth-child(3)")!.click(); await settle();
    const input = target.querySelector<HTMLInputElement>('[aria-label="Search Spotify"]')!;
    input.value = "new query"; input.dispatchEvent(new Event("input", { bubbles: true }));
    target.querySelector<HTMLButtonElement>('.remote-search-form button[type="submit"]')!.click(); await settle();
    account.resolve({ accountRevisions: { spotify: 2 }, spotifyAccountId: "fixture" }); await settle();
    expect(input.value).toBe("new query");
    expect(target.textContent).toContain("Fresh query result");
    expect(call).toHaveBeenCalledWith("search_remote_playlists", expect.objectContaining({ query: "new query" }));
  });

  it("stops an evicted-page restore after the user changes the source query", async () => {
    const status = { accountRevisions: { spotify: 4 }, spotifyAccountId: "fixture" };
    const first = vi.fn(async (command: string, args?: Record<string, unknown>) => {
      if (command === "get_managed_provider_session_status") return status;
      if (command === "list_remote_playlist_bindings") return [];
      if (command === "list_remote_playlists") return args?.cursor
        ? { items: [{ id: "second", title: "Second saved" }], nextCursor: null, complete: true }
        : { items: [{ id: "first", title: "First saved" }], nextCursor: "saved-page-2", complete: false };
      throw new Error(`Unexpected command ${command}`);
    });
    const original = render(first); await settle();
    original.querySelector<HTMLButtonElement>(".remote-more")!.click(); await settle();
    destroy?.(); destroy = undefined;

    const delayed = deferred<unknown>();
    const restoredCall = vi.fn(async (command: string, args?: Record<string, unknown>) => {
      if (command === "get_managed_provider_session_status") return { ...status, accountRevisions: { spotify: 5 } };
      if (command === "list_remote_playlist_bindings") return [];
      if (command === "list_remote_playlists") return delayed.promise;
      if (command === "search_remote_playlists") return { items: [{ id: "new", title: "New search" }] };
      throw new Error(`Unexpected command ${command}`);
    });
    const target = render(restoredCall); await settle();
    expect(restoredCall).toHaveBeenCalledWith("list_remote_playlists", expect.objectContaining({ provider: "spotify" }));
    const tab = target.querySelector<HTMLButtonElement>(".remote-source-tabs button:nth-child(3)")!; tab.click(); await settle();
    const input = target.querySelector<HTMLInputElement>('[aria-label="Search Spotify"]')!;
    input.value = "changed"; input.dispatchEvent(new Event("input", { bubbles: true }));
    target.querySelector<HTMLButtonElement>('.remote-search-form button[type="submit"]')!.click(); await settle();
    delayed.resolve({ items: [{ id: "late", title: "Late old browse" }], nextCursor: "old-next" }); await settle();
    expect(restoredCall.mock.calls.filter(([command]) => command === "list_remote_playlists")).toHaveLength(1);
    expect(target.textContent).toContain("New search");
    expect(target.textContent).not.toContain("Late old browse");
  });

  it("pauses an evicted profile restore after two minutes and reports the rows still missing", async () => {
    vi.useFakeTimers();
    vi.setSystemTime(new Date("2026-10-16T12:00:00Z"));
    const snapshot = {
      provider: "spotify" as const, sourceMode: "profile" as const, profileUrl: "profile", directUrl: "", playlistUrls: [],
      query: "", searchKind: "playlist" as const, items: [], cursor: "page-1" as string | null, selected: [], seenCursors: [],
      currentBrowseComplete: false, pageNote: "", scroll: 0, loadedCount: 600, needsRefetch: true,
    };
    providerNavigationSnapshots.put({
      kind: "playlists", key: JSON.stringify(["spotify", "profile"]), accountScope: "spotify:revision:4:fixture",
      snapshot, rows: 0, stripRows: value => ({ ...value, items: [], cursor: null, selected: [], needsRefetch: true }),
    });
    let page = 0;
    const call = vi.fn(async (command: string) => {
      if (command === "get_managed_provider_session_status") return { accountRevisions: { spotify: 4 }, spotifyAccountId: "fixture" };
      if (command === "list_remote_playlist_bindings") return [];
      if (command === "list_remote_playlists") {
        page += 1;
        vi.setSystemTime(Date.now() + 30_000);
        return { items: [{ id: `playlist-${page}`, title: `Playlist ${page}` }], nextCursor: `page-${page + 1}` };
      }
      throw new Error(`Unexpected command ${command}`);
    });
    const target = render(call, { initialOwner: "profile" });
    for (let step = 0; step < 100 && !target.textContent?.includes("restoring paused after two minutes"); step++) { await Promise.resolve(); await tick(); }
    expect(page).toBe(4);
    expect(target.querySelectorAll(".remote-result-row")).toHaveLength(3);
    expect(target.textContent).toContain("restoring paused after two minutes");
    expect(target.textContent).toContain("Load more to continue");
  });

  it("selects all 300 profile results across ordinary pages within the bounded collection", async () => {
    const rows = Array.from({ length: 300 }, (_, index) => ({ id: `profile-playlist-${index + 1}`, title: `Profile playlist ${index + 1}` }));
    const call = vi.fn(async (command: string, args?: Record<string, unknown>) => {
      if (command === "list_remote_playlist_bindings") return [];
      if (command !== "list_remote_playlists") throw new Error(`Unexpected command ${command}`);
      const page = Number(String(args?.cursor || "0").replace("page-", ""));
      const start = page * 30;
      return { items: rows.slice(start, start + 30), nextCursor: start + 30 < rows.length ? `page-${page + 1}` : null, totalCount: rows.length, complete: start + 30 >= rows.length };
    });
    const target = render(call, { initialOwner: "profile" }); await settle();
    target.querySelector<HTMLButtonElement>(".remote-selection-actions button")!.click(); await settle();
    await vi.waitFor(() => expect(target.textContent).toContain("Selected all 300 playlists in this browse."));
    expect(call.mock.calls.filter(([command]) => command === "list_remote_playlists")).toHaveLength(10);
    expect(target.querySelectorAll(".remote-result-row")).toHaveLength(300);
    expect(target.textContent).toContain("Selected all 300 playlists in this browse.");
    expect(target.textContent).toContain("300 selected");
  });

  it("continues select all beyond the former 100-page cap", async () => {
    const call = vi.fn(async (command: string, args?: Record<string, unknown>) => {
      if (command === "list_remote_playlist_bindings") return [];
      if (command !== "list_remote_playlists") throw new Error(`Unexpected command ${command}`);
      const page = Number(String(args?.cursor || "0").replace("page-", ""));
      return { items: [{ id: `long-${page}`, title: `Playlist ${page}` }], nextCursor: page < 101 ? `page-${page + 1}` : null, complete: page === 101 };
    });
    const target = render(call, { initialOwner: "profile" }); await settle();
    target.querySelector<HTMLButtonElement>(".remote-selection-actions button")!.click(); await settle();
    await vi.waitFor(() => expect(target.textContent).toContain("Selected all 102 playlists in this browse."));
    expect(call.mock.calls.filter(([command]) => command === "list_remote_playlists")).toHaveLength(102);
    expect(target.querySelectorAll(".remote-result-row")).toHaveLength(102);
    expect(target.textContent).toContain("102 selected");
  });

  it("does not commit a page that would exceed the hard collection bound", () => {
    const current = Array.from({ length: 9_999 }, (_, index) => ({ id: `bounded-${index}` }));
    expect(appendPageWithinLimit(current, [{ id: "overflow-a" }, { id: "overflow-b" }], 10_000)).toBeNull();
    expect(current).toHaveLength(9_999);
    expect(appendPageWithinLimit(current, [{ id: "duplicate-bounded-0" }, { id: "bounded-0" }], 10_000)).toHaveLength(10_000);
  });

  it("marks select all incomplete when the two-minute budget expires", async () => {
    vi.useFakeTimers();
    vi.setSystemTime(new Date("2026-10-16T12:00:00Z"));
    let page = 0;
    const call = vi.fn(async (command: string, args?: Record<string, unknown>) => {
      if (command === "list_remote_playlist_bindings") return [];
      if (command !== "list_remote_playlists") throw new Error(`Unexpected command ${command}`);
      if (args?.cursor) {
        page += 1;
        vi.setSystemTime(Date.now() + 30_000);
      }
      return { items: [{ id: `timed-${page}`, title: `Timed ${page}` }], nextCursor: `page-${page + 1}`, complete: false };
    });
    const target = render(call, { initialOwner: "profile" }); await settle();
    target.querySelector<HTMLButtonElement>(".remote-selection-actions button")!.click();
    for (let step = 0; step < 100 && !target.textContent?.includes("Select all paused after two minutes"); step++) { await Promise.resolve(); await tick(); }
    expect(page).toBe(4);
    expect(target.textContent).toContain("Select all paused after two minutes");
    expect(target.textContent).toContain("more playlists may remain");
    expect(target.querySelector<HTMLButtonElement>(".remote-more")).not.toBeNull();
  });

  it("cancels an in-flight select-all page at the two-minute deadline and ignores its late result", async () => {
    vi.useFakeTimers();
    const pending = deferred<unknown>();
    const call = vi.fn(async (command: string, args?: Record<string, unknown>) => {
      if (command === "list_remote_playlist_bindings") return [];
      if (command === "cancel_remote_read") return { cancelled: true };
      if (command === "list_remote_playlists") return args?.cursor
        ? pending.promise
        : { items: [{ id: "first", title: "First" }], nextCursor: "page-2", complete: false };
      throw new Error(`Unexpected command ${command}`);
    });
    const target = render(call, { initialOwner: "profile" }); await settle();
    target.querySelector<HTMLButtonElement>(".remote-selection-actions button")!.click(); await settle();
    await vi.advanceTimersByTimeAsync(120_000); await settle();
    expect(call).toHaveBeenCalledWith("cancel_remote_read", expect.objectContaining({ requestId: expect.any(String) }));
    expect(target.textContent).toContain("Select all paused after two minutes");
    pending.resolve({ items: [{ id: "late", title: "Late result" }], complete: true }); await settle();
    expect(target.textContent).not.toContain("Late result");
    expect(target.querySelectorAll(".remote-result-row")).toHaveLength(1);
  });

  it("does not let an expired stale restore cancel a newer search read", async () => {
    vi.useFakeTimers();
    const stalePage = deferred<unknown>();
    const currentSearch = deferred<unknown>();
    const status = { accountRevisions: { spotify: 8 }, spotifyAccountId: "fixture" };
    const snapshot = {
      provider: "spotify" as const, sourceMode: "profile" as const, profileUrl: "profile", directUrl: "", playlistUrls: [],
      query: "", searchKind: "playlist" as const, items: [], cursor: null, selected: [], seenCursors: [],
      currentBrowseComplete: false, pageNote: "", snapshotConsistency: null, scroll: 0, loadedCount: 1, needsRefetch: true,
    };
    providerNavigationSnapshots.put({ kind: "playlists", key: JSON.stringify(["spotify", "profile"]), accountScope: "spotify:revision:8:fixture", snapshot, rows: 0, stripRows: value => ({ ...value, items: [], cursor: null, selected: [], needsRefetch: true }) });
    const call = vi.fn(async (command: string, args?: Record<string, unknown>) => {
      if (command === "get_managed_provider_session_status") return status;
      if (command === "list_remote_playlist_bindings") return [];
      if (command === "cancel_remote_read") return { cancelled: true };
      if (command === "list_remote_playlists") return stalePage.promise;
      if (command === "search_remote_playlists") return currentSearch.promise;
      throw new Error(`Unexpected command ${command}`);
    });
    const target = render(call, { initialOwner: "profile" }); await settle();
    const staleRequestId = call.mock.calls.find(([command]) => command === "list_remote_playlists")?.[1]?.requestId;
    target.querySelector<HTMLButtonElement>(".remote-source-tabs button:nth-child(3)")!.click(); await settle();
    const input = target.querySelector<HTMLInputElement>('[aria-label="Search Spotify"]')!;
    input.value = "current query"; input.dispatchEvent(new Event("input", { bubbles: true }));
    target.querySelector<HTMLButtonElement>('.remote-search-form button[type="submit"]')!.click(); await settle();
    const searchRequestId = call.mock.calls.find(([command]) => command === "search_remote_playlists")?.[1]?.requestId;
    expect(searchRequestId).toEqual(expect.any(String));
    expect(call).toHaveBeenCalledWith("cancel_remote_read", { requestId: staleRequestId });

    await vi.advanceTimersByTimeAsync(120_000); await settle();
    expect(call.mock.calls.filter(([command, args]) => command === "cancel_remote_read" && args?.requestId === searchRequestId)).toHaveLength(0);
    currentSearch.resolve({ items: [{ id: "current-result", title: "Current result" }] }); await settle();
    expect(target.textContent).toContain("Current result");
    stalePage.resolve({ items: [{ id: "stale-result", title: "Stale result" }], complete: true }); await settle();
    expect(target.textContent).not.toContain("Stale result");
    expect(target.textContent).toContain("Current result");
  });

  it("cancels an in-flight evicted-page restore at its two-minute deadline", async () => {
    vi.useFakeTimers();
    const status = { accountRevisions: { spotify: 7 }, spotifyAccountId: "fixture" };
    const snapshot = {
      provider: "spotify" as const, sourceMode: "profile" as const, profileUrl: "profile", directUrl: "", playlistUrls: [],
      query: "", searchKind: "playlist" as const, items: [], cursor: null, selected: [], seenCursors: [],
      currentBrowseComplete: false, pageNote: "", snapshotConsistency: null, scroll: 0, loadedCount: 1, needsRefetch: true,
    };
    providerNavigationSnapshots.put({ kind: "playlists", key: JSON.stringify(["spotify", "profile"]), accountScope: "spotify:revision:7:fixture", snapshot, rows: 0, stripRows: value => ({ ...value, items: [], cursor: null, selected: [], needsRefetch: true }) });
    const pending = deferred<unknown>();
    const call = vi.fn(async (command: string, args?: Record<string, unknown>) => {
      if (command === "get_managed_provider_session_status") return status;
      if (command === "list_remote_playlist_bindings") return [];
      if (command === "cancel_remote_read") return { cancelled: true };
      if (command === "list_remote_playlists") return pending.promise;
      throw new Error(`Unexpected command ${command}`);
    });
    const target = render(call, { initialOwner: "profile" }); await settle();
    await vi.advanceTimersByTimeAsync(120_000); await settle();
    expect(call).toHaveBeenCalledWith("cancel_remote_read", expect.objectContaining({ requestId: expect.any(String) }));
    expect(target.textContent).toContain("restoring paused after two minutes");
    pending.resolve({ items: [{ id: "late-restore", title: "Late restore" }], complete: true }); await settle();
    expect(target.textContent).not.toContain("Late restore");
  });

  it("restores playlist position on the shared result scroll surface", async () => {
    const status = { accountRevisions: { spotify: 4 }, spotifyAccountId: "fixture" };
    const first = vi.fn(async (command: string) => {
      if (command === "get_managed_provider_session_status") return status;
      if (command === "list_remote_playlist_bindings") return [];
      if (command === "list_remote_playlists") return { items: [{ id: "one", title: "One" }, { id: "two", title: "Two" }], complete: true };
      throw new Error(`Unexpected command ${command}`);
    });
    const original = render(first, { initialOwner: "profile" }); await settle();
    const scrollSurface = original.querySelector<HTMLElement>(".remote-playlists")!;
    scrollSurface.scrollTop = 46;
    scrollSurface.dispatchEvent(new Event("scroll"));
    destroy?.(); destroy = undefined;

    const restoredCall = vi.fn(async (command: string) => {
      if (command === "get_managed_provider_session_status") return status;
      if (command === "list_remote_playlist_bindings") return [];
      throw new Error(`Snapshot restore unexpectedly browsed: ${command}`);
    });
    const restored = render(restoredCall, { initialOwner: "profile" }); await settle();
    expect(restored.textContent).toContain("One");
    expect(restored.querySelector<HTMLElement>(".remote-playlists")?.scrollTop).toBe(46);
    expect(restoredCall).not.toHaveBeenCalledWith("list_remote_playlists", expect.anything());
  });

  it("ignores a browse response after the source mode changes", async () => {
    const firstPage = deferred<unknown>();
    const call = vi.fn(async (command: string, _args?: Record<string, unknown>) => {
      if (command === "list_remote_playlist_bindings") return [];
      if (command === "list_remote_playlists") return firstPage.promise;
      throw new Error(`Unexpected command ${command}`);
    });
    const target = render(call); await settle();
    const staleRequestId = call.mock.calls.find(([command]) => command === "list_remote_playlists")?.[1]?.requestId;
    target.querySelector<HTMLButtonElement>(".remote-source-tabs button:nth-child(3)")!.click();
    expect(call).toHaveBeenCalledWith("cancel_remote_read", { requestId: staleRequestId });
    firstPage.resolve({ items: [{ id: "late-mine", title: "Late Mine result" }], nextCursor: "old-cursor" });
    await settle();
    expect(target.textContent).not.toContain("Late Mine result");
    expect(target.querySelector(".remote-more")).toBeNull();
    expect(target.textContent).not.toContain("Loading playlists…");
  });

  it("ignores old-provider browse and binding responses after switching providers", async () => {
    const oldPage = deferred<unknown>();
    const oldBindings = deferred<unknown>();
    const call = vi.fn(async (command: string, args?: Record<string, unknown>) => {
      if (command === "list_remote_playlist_bindings") return args?.provider === "spotify" ? oldBindings.promise : [];
      if (command === "list_remote_playlists") return args?.provider === "spotify" ? oldPage.promise : { items: [{ id: "youtube-row", title: "YouTube row" }] };
      throw new Error(`Unexpected command ${command}`);
    });
    const target = render(call); await settle();
    target.querySelector<HTMLButtonElement>('.remote-provider-tabs button[aria-pressed="false"]')!.click();
    await settle();
    const oldBrowseId = call.mock.calls.find(([command, args]) => command === "list_remote_playlists" && (args as Record<string, unknown>)?.provider === "spotify")?.[1]?.requestId;
    expect(call).toHaveBeenCalledWith("cancel_remote_read", { requestId: oldBrowseId });
    oldBindings.resolve([{ playlistId: "youtube-row" }]);
    oldPage.resolve({ items: [{ id: "late-spotify", title: "Late Spotify row" }] });
    await settle();
    expect(target.textContent).toContain("YouTube row");
    expect(target.textContent).not.toContain("Late Spotify row");
    expect(target.textContent).not.toContain("Imported");
  });

  it("ignores a pending search when the query changes", async () => {
    const oldSearch = deferred<unknown>();
    const call = vi.fn(async (command: string, args?: Record<string, unknown>) => {
      if (command === "list_remote_playlist_bindings") return [];
      if (command === "list_remote_playlists") return { items: [], nextCursor: null };
      if (command === "search_remote_playlists") return args?.query === "old query" ? oldSearch.promise : { items: [{ id: "new-result", title: "New query result" }] };
      throw new Error(`Unexpected command ${command}`);
    });
    const target = render(call); await settle();
    target.querySelector<HTMLButtonElement>(".remote-source-tabs button:nth-child(3)")!.click();
    await settle();
    const input = target.querySelector<HTMLInputElement>('[aria-label="Search Spotify"]')!;
    input.value = "old query"; input.dispatchEvent(new Event("input", { bubbles: true })); await settle();
    target.querySelector<HTMLButtonElement>('.remote-search-form button[type="submit"]')!.click(); await settle();
    const oldSearchId = call.mock.calls.find(([command, args]) => command === "search_remote_playlists" && (args as Record<string, unknown>)?.query === "old query")?.[1]?.requestId;
    input.value = "new query"; input.dispatchEvent(new Event("input", { bubbles: true })); await settle();
    expect(call).toHaveBeenCalledWith("cancel_remote_read", { requestId: oldSearchId });
    oldSearch.resolve({ items: [{ id: "late-result", title: "Old query result" }], nextCursor: "stale-cursor" });
    await settle();
    expect(target.textContent).not.toContain("Old query result");
    expect(target.querySelector(".remote-more")).toBeNull();
    expect(target.textContent).not.toContain("Loading playlists…");
  });

  it("stops select-all without mixing pages when its captured descriptor changes", async () => {
    const secondPage = deferred<unknown>();
    const call = vi.fn(async (command: string, args?: Record<string, unknown>) => {
      if (command === "list_remote_playlist_bindings") return [];
      if (command === "list_remote_playlists") return { items: [], nextCursor: null };
      if (command === "search_remote_playlists") return args?.cursor
        ? secondPage.promise
        : { items: [{ id: "first-result", title: "First result" }], nextCursor: "page-two" };
      throw new Error(`Unexpected command ${command}`);
    });
    const target = render(call); await settle();
    target.querySelector<HTMLButtonElement>(".remote-source-tabs button:nth-child(3)")!.click();
    await settle();
    const input = target.querySelector<HTMLInputElement>('[aria-label="Search Spotify"]')!;
    input.value = "first query"; input.dispatchEvent(new Event("input", { bubbles: true })); await settle();
    target.querySelector<HTMLButtonElement>('.remote-search-form button[type="submit"]')!.click(); await settle();
    target.querySelector<HTMLButtonElement>(".remote-selection-actions button")!.click(); await settle();
    input.disabled = false;
    input.value = "changed query"; input.dispatchEvent(new Event("input", { bubbles: true })); await settle();
    secondPage.resolve({ items: [{ id: "mixed-result", title: "Must not be mixed" }], nextCursor: null });
    await settle();
    expect(target.textContent).not.toContain("Must not be mixed");
    expect(target.textContent).not.toContain("First result");
    expect(target.textContent).toContain("0 selected");
    expect(target.textContent).not.toContain("Selected all");
  });

  it("cancels its in-flight select-all read and keeps only the already loaded selection", async () => {
    const secondPage = deferred<unknown>();
    const call = vi.fn(async (command: string, args?: Record<string, unknown>) => {
      if (command === "list_remote_playlist_bindings") return [];
      if (command === "list_remote_playlists") return args?.cursor
        ? secondPage.promise
        : { items: [{ id: "loaded-a", title: "Loaded A" }], nextCursor: "page-two" };
      throw new Error(`Unexpected command ${command}`);
    });
    const target = render(call); await settle();
    target.querySelector<HTMLButtonElement>(".remote-selection-actions button")!.click(); await settle();
    const secondPageId = call.mock.calls.find(([command, args]) => command === "list_remote_playlists" && (args as Record<string, unknown>)?.cursor === "page-two")?.[1]?.requestId;
    target.querySelector<HTMLButtonElement>(".remote-cancel-import")!.click();
    expect(call).toHaveBeenCalledWith("cancel_remote_read", { requestId: secondPageId });
    secondPage.resolve({ items: [{ id: "late-b", title: "Late B" }], nextCursor: null, complete: true });
    await settle();
    expect(target.textContent).toContain("Selected 1 playlists before stopping.");
    expect(target.textContent).toContain("1 selected");
    expect(target.textContent).not.toContain("Late B");
    expect(call).not.toHaveBeenCalledWith("import_remote_playlists", expect.anything());
  });

  it("loads and multi-selects existing playlists, then imports the selected IDs", async () => {
    const call = vi.fn(async (command: string) => {
      if (command === "list_remote_playlist_bindings") return [];
      if (command === "list_remote_playlists") return { items: [
        { id: "mix-a", name: "Late Set", ownerName: "DJ Example", trackCount: 14, visibility: "Public" },
        { id: "mix-b", name: "Warmup", ownerName: "DJ Example", trackCount: 8, visibility: "Private" },
      ], nextCursor: null };
      if (command === "import_remote_playlists") return { state: "completed", completed: 2, total: 2, results: [
        { bindingId: "binding-a", playlistId: "mix-a", collectionId: "local-a", title: "Late Set", trackCount: 14, complete: true },
        { bindingId: "binding-b", playlistId: "mix-b", collectionId: "local-b", title: "Warmup", trackCount: 8, complete: true },
      ], errors: [] };
      throw new Error(`Unexpected command ${command}`);
    });
    const target = render(call); await settle();
    expect(target.textContent).toContain("14 tracks · Public");
    expect(target.textContent).toContain("DJ Example");
    target.querySelector<HTMLInputElement>('[aria-label="Select Late Set"]')!.click();
    target.querySelector<HTMLInputElement>('[aria-label="Select Warmup"]')!.click();
    await settle();
    expect(target.textContent).toContain("2 selected");
    target.querySelector<HTMLButtonElement>(".remote-import-footer .primary")!.click(); await settle();
    expect(call).toHaveBeenCalledWith("import_remote_playlists", { provider: "spotify", playlistIds: ["mix-a", "mix-b"] });
    expect(target.textContent).toContain("2 playlists imported.");
  });

  it("lets long remote playlist titles wrap instead of clipping", async () => {
    const title = "Salsa Greatest Hits | Songs for the full dance floor and the late set";
    const call = vi.fn(async (command: string) => {
      if (command === "list_remote_playlist_bindings") return [];
      if (command === "list_remote_playlists") return { items: [{ id: "salsa", title }], nextCursor: null };
      throw new Error(`Unexpected command ${command}`);
    });
    const target = render(call); await settle();
    const titleButton = target.querySelector<HTMLButtonElement>(".remote-result-title")!;
    expect(titleButton.textContent).toBe(title);
  });

  it("does not present a bounded no-cursor profile result as exhaustive", async () => {
    const call = vi.fn(async (command: string) => {
      if (command === "list_remote_playlist_bindings") return [];
      if (command === "list_remote_playlists") return {
        items: [{ id: "profile-playlist", title: "Visible playlist" }], nextCursor: null,
        totalCount: 64, truncated: true, complete: false, snapshotConsistency: "progressive",
      };
      throw new Error(`Unexpected command ${command}`);
    });
    const target = render(call, { initialOwner: "public-profile" }); await settle();
    expect(target.textContent).toContain("bounded or incomplete result");
    expect(target.textContent).toContain("64 total items were reported");
    expect(target.textContent).toContain("Playlists load progressively; refresh to reflect changes.");
    expect(target.querySelectorAll(".remote-notice")).toHaveLength(1);
    expect(target.textContent).not.toContain("Selected all");
    target.querySelector<HTMLButtonElement>(".remote-selection-actions button")!.click(); await settle();
    expect(target.textContent).toContain("Selected the 1 playlists returned in this browse. The provider did not confirm the list is complete.");
    expect(target.textContent).not.toContain("Selected all");
  });

  it("claims exhaustive selection only when the provider explicitly marks a no-cursor page complete", async () => {
    const call = vi.fn(async (command: string, _args?: Record<string, unknown>) => {
      if (command === "list_remote_playlist_bindings") return [];
      if (command === "list_remote_playlists") return { items: [{ id: "complete-a", title: "Complete A" }], nextCursor: null, complete: true };
      throw new Error(`Unexpected command ${command}`);
    });
    const target = render(call); await settle();
    target.querySelector<HTMLButtonElement>(".remote-selection-actions button")!.click(); await settle();
    expect(target.textContent).toContain("Selected all 1 playlists in this browse.");
  });

  it("keeps source sync controls out of the browse picker", async () => {
    const call = vi.fn(async (command: string) => {
      if (command === "list_remote_playlist_bindings") return [{ bindingId: "binding-a", provider: "spotify", playlistId: "mix-a", title: "Late Set", canEdit: true }];
      if (command === "list_remote_playlists") return { items: [], nextCursor: null };
      throw new Error(`Unexpected command ${command}`);
    });
    const target = render(call); await settle();
    expect(target.querySelector(".remote-sync-panel")).toBeNull();
    expect(target.querySelectorAll("button").length).toBeGreaterThan(0);
    expect(call).not.toHaveBeenCalledWith("get_remote_playlist_sync_preview", expect.anything());
  });

  it("shows job progress and sends an explicit cancel request", async () => {
    vi.useFakeTimers();
    const call = vi.fn(async (command: string) => {
      if (command === "list_remote_playlist_bindings") return [];
      if (command === "list_remote_playlists") return { items: [{ id: "mix-a", name: "Late Set" }], nextCursor: null };
      if (command === "import_remote_playlists") return { jobId: "job-1", state: "queued", completed: 0, total: 1, results: [], errors: [] };
      if (command === "cancel_remote_playlist_import") return { jobId: "job-1", state: "cancelled", completed: 0, total: 1, results: [], errors: [] };
      if (command === "get_remote_playlist_import_job") return { jobId: "job-1", state: "cancelled", completed: 0, total: 1, results: [], errors: [] };
      throw new Error(`Unexpected command ${command}`);
    });
    const target = render(call); await settle();
    target.querySelector<HTMLInputElement>('[aria-label="Select Late Set"]')!.click(); await settle();
    target.querySelector<HTMLButtonElement>(".remote-import-footer .primary")!.click(); await settle();
    expect(target.textContent).toContain("Importing 0 of 1");
    target.querySelector<HTMLButtonElement>(".remote-cancel-import")!.click(); await settle();
    expect(call).toHaveBeenCalledWith("cancel_remote_playlist_import", { jobId: "job-1" });
    await vi.advanceTimersByTimeAsync(800); await settle();
    expect(target.textContent).toContain("Import canceled. 0 playlists completed");
  });

  it("opens the public profile source for an initial owner", async () => {
    const call = vi.fn(async (command: string, args?: Record<string, unknown>) => {
      if (command === "list_remote_playlist_bindings") return [];
      if (command === "list_remote_playlists") return { provider: "spotify", items: [], nextCursor: null };
      throw new Error(`Unexpected command ${command}`);
    });
    render(call, { initialOwner: "artist-profile-1" }); await settle();
    expect(call).toHaveBeenCalledWith("list_remote_playlists", expect.objectContaining({ provider: "spotify", ownerId: "artist-profile-1", requestId: expect.any(String) }));
  });

  it("accepts official Spotify profile URLs with dotted, underscored, and hyphenated IDs", async () => {
    const call = vi.fn(async (command: string) => {
      if (command === "list_remote_playlist_bindings") return [];
      if (command === "list_remote_playlists") return { items: [], nextCursor: null };
      throw new Error(`Unexpected command ${command}`);
    });
    const url = "https://open.spotify.com/user/dj_name.one-2";
    render(call, { initialOwner: url }); await settle();
    expect(call).toHaveBeenCalledWith("list_remote_playlists", expect.objectContaining({ provider: "spotify", ownerId: url, requestId: expect.any(String) }));
  });

  it("stops loading when a provider repeats the current page cursor", async () => {
    const call = vi.fn(async (command: string, args?: Record<string, unknown>) => {
      if (command === "list_remote_playlist_bindings") return [];
      if (command === "list_remote_playlists") return args?.cursor
        ? { items: [{ id: "a", title: "Playlist A" }], nextCursor: "page-2" }
        : { items: [{ id: "a", title: "Playlist A" }], nextCursor: "page-2" };
      throw new Error(`Unexpected command ${command}`);
    });
    const target = render(call); await settle();
    target.querySelector<HTMLButtonElement>(".remote-more")!.click(); await settle();
    expect(call.mock.calls.filter(([command]) => command === "list_remote_playlists")).toHaveLength(2);
    expect(target.textContent).toContain("provider repeated a playlist page");
    expect(target.querySelector(".remote-more")).toBeNull();
  });

  it("does not show a progressive Load more notice after a repeated cursor stops paging", async () => {
    const call = vi.fn(async (command: string, args?: Record<string, unknown>) => {
      if (command === "list_remote_playlist_bindings") return [];
      if (command === "list_remote_playlists") return {
        items: [{ id: args?.cursor ? "b" : "a", title: args?.cursor ? "Playlist B" : "Playlist A" }],
        nextCursor: "page-2", totalCount: 3, complete: false, snapshotConsistency: "progressive",
      };
      throw new Error(`Unexpected command ${command}`);
    });
    const target = render(call, { initialOwner: "https://open.spotify.com/user/example" }); await settle();
    expect(target.textContent).toContain("Showing 1 of 3 playlists. Load more to continue.");
    target.querySelector<HTMLButtonElement>(".remote-more")!.click(); await settle();
    expect(target.textContent).toContain("provider repeated a playlist page");
    expect(target.textContent).toContain("loaded set may be incomplete");
    expect(target.querySelector(".remote-more")).toBeNull();
    expect(target.textContent).not.toContain("Load more to continue.");
  });

  it("searches Spotify profile names and waits for an explicit choice", async () => {
    const call = vi.fn(async (command: string, args?: Record<string, unknown>) => {
      if (command === "list_remote_playlist_bindings") return [];
      if (command === "list_remote_playlists") return { items: [], nextCursor: null };
      if (command === "search_source") return { results: [
        { id: "profile-1", kind: "profile", title: "Andrew Kelsh", subtitle: "Spotify profile", url: "https://open.spotify.com/user/profile1" },
        { id: "profile-2", kind: "profile", title: "Andrew Kells", subtitle: "Another profile", url: "https://open.spotify.com/user/profile2" },
      ] };
      throw new Error(`Unexpected command ${command}`);
    });
    const target = render(call, { initialOwner: "Andrew Kalsh" }); await settle();
    [...target.querySelectorAll<HTMLButtonElement>("button")].find(button => button.textContent?.includes("Search profiles"))!.click(); await settle();
    expect(call).toHaveBeenCalledWith("search_source", expect.objectContaining({ query: "Andrew Kalsh", sourceIds: ["spotify"], kinds: ["profile"], cursor: null, limit: 20, requestId: expect.any(String) }));
    expect(target.querySelectorAll(".remote-profile-choices li")).toHaveLength(2);
    expect(call).not.toHaveBeenCalledWith("list_remote_playlists", { provider: "spotify", ownerId: "https://open.spotify.com/user/profile1" });
    target.querySelectorAll<HTMLButtonElement>(".remote-profile-choices button")[1].click(); await settle();
    expect(call).toHaveBeenCalledWith("list_remote_playlists", expect.objectContaining({ provider: "spotify", ownerId: "https://open.spotify.com/user/profile2", requestId: expect.any(String) }));
  });

  it("discards a Spotify profile search that finishes after leaving profile mode", async () => {
    let finishSearch!: (value: unknown) => void;
    const pendingSearch = new Promise(resolve => { finishSearch = resolve; });
    const call = vi.fn(async (command: string) => {
      if (command === "list_remote_playlist_bindings") return [];
      if (command === "search_source") return pendingSearch;
      if (command === "list_remote_playlists") return { items: [], nextCursor: null };
      throw new Error(`Unexpected command ${command}`);
    });
    const target = render(call, { initialOwner: "Andrew Kalsh" }); await settle();
    [...target.querySelectorAll<HTMLButtonElement>("button")].find(button => button.textContent?.includes("Search profiles"))!.click();
    await settle();
    [...target.querySelectorAll<HTMLButtonElement>(".remote-source-tabs button")].find(button => button.textContent === "Search")!.click();
    finishSearch({ results: [{ id: "profile-late", kind: "profile", title: "Late result", url: "https://open.spotify.com/user/profile-late" }] });
    await settle();
    expect(target.querySelector(".remote-profile-choices")).toBeNull();
    expect(target.textContent).not.toContain("Late result");
  });

  it("shows known zero track counts, hides unknown metadata, and opens creator links externally", async () => {
    const call = vi.fn(async (command: string) => {
      if (command === "list_remote_playlist_bindings") return [];
      if (command === "list_remote_playlists") return { items: [
        { id: "zero", name: "Empty playlist", ownerName: "Owner Zero", ownerUrl: "https://open.spotify.com/user/ownerzero", trackCount: 0, visibility: null },
        { id: "unknown", name: "Unknown playlist", ownerName: "Owner Unknown", ownerUrl: "https://open.spotify.com/user/ownerunknown", trackCount: null, visibility: null },
      ], nextCursor: null };
      if (command === "open_external_url") return null;
      throw new Error(`Unexpected command ${command}`);
    });
    const target = render(call); await settle();
    expect(target.textContent).toContain("0 tracks");
    expect(target.textContent).not.toContain("null tracks");
    expect(target.textContent).not.toContain("Visibility unavailable");
    target.querySelector<HTMLButtonElement>('[aria-label="Open Owner Zero profile in browser"]')!.click(); await settle();
    expect(call).toHaveBeenCalledWith("open_external_url", { url: "https://open.spotify.com/user/ownerzero" });
  });

  it("routes only validated Spotify user profiles through owner browsing", async () => {
    const call = vi.fn(async (command: string) => {
      if (command === "list_remote_playlist_bindings") return [];
      if (command === "list_remote_playlists") return { items: [
        { id: "valid", name: "Valid owner playlist", ownerName: "Owner Zero", ownerUrl: "https://open.spotify.com/user/joe.smith/" },
        { id: "encoded-name", name: "Legacy owner playlist", ownerName: "Legacy User", ownerUrl: "https://open.spotify.com/user/Andrew.Kal%C5%A1" },
        { id: "invalid", name: "Unsupported owner route", ownerName: "Other Owner", ownerId: null, ownerUrl: "https://open.spotify.com/artist/not-a-user" },
        { id: "encoded-slash", name: "Unsafe encoded path", ownerName: "Unsafe Owner", ownerId: null, ownerUrl: "https://open.spotify.com/user/joe%2Fadmin" },
      ], complete: true };
      throw new Error(`Unexpected command ${command}`);
    });
    const onBrowseOwner = vi.fn();
    const target = render(call, { onBrowseOwner }); await settle();
    const owner = target.querySelector<HTMLButtonElement>('[aria-label="Browse Owner Zero playlists"]');
    expect(owner?.title).toBe("Browse Owner Zero playlists");
    owner!.click(); await settle();
    expect(onBrowseOwner).toHaveBeenCalledWith("spotify", "https://open.spotify.com/user/joe.smith");
    target.querySelector<HTMLButtonElement>('[aria-label="Browse Legacy User playlists"]')!.click(); await settle();
    expect(onBrowseOwner).toHaveBeenCalledWith("spotify", "https://open.spotify.com/user/Andrew.Kal%C5%A1");
    expect(call).not.toHaveBeenCalledWith("open_external_url", expect.anything());
    expect(target.textContent).toContain("Other Owner");
    expect(target.textContent).toContain("Unsafe Owner");
    expect([...target.querySelectorAll<HTMLButtonElement>(".remote-creator-link")].map(button => button.textContent)).toEqual(["Owner Zero", "Legacy User"]);
    expect(onBrowseOwner).toHaveBeenCalledTimes(2);
  });

  it("keeps YouTube creator links external when owner browsing is available", async () => {
    const call = vi.fn(async (command: string, args?: Record<string, unknown>) => {
      if (command === "list_remote_playlist_bindings") return [];
      if (command === "list_remote_playlists") return args?.provider === "youtube"
        ? { items: [{ id: "yt", name: "Channel mix", ownerName: "Example Channel", ownerUrl: "https://www.youtube.com/@example" }], complete: true }
        : { items: [], complete: true };
      if (command === "open_external_url") return null;
      throw new Error(`Unexpected command ${command}`);
    });
    const onBrowseOwner = vi.fn();
    const target = render(call, { onBrowseOwner }); await settle();
    target.querySelector<HTMLButtonElement>('.remote-provider-tabs button[aria-pressed="false"]')!.click(); await settle();
    target.querySelector<HTMLButtonElement>('[aria-label="Open Example Channel profile in browser"]')!.click(); await settle();
    expect(call).toHaveBeenCalledWith("open_external_url", { url: "https://www.youtube.com/@example" });
    expect(onBrowseOwner).not.toHaveBeenCalled();
  });

  it("selects and deduplicates every page without importing until explicitly requested", async () => {
    const call = vi.fn(async (command: string, args?: Record<string, unknown>) => {
      if (command === "list_remote_playlist_bindings") return [];
      if (command === "list_remote_playlists") {
        const cursor = args?.cursor;
        if (!cursor) return { items: [{ id: "a", title: "A" }, { id: "b", title: "B" }], nextCursor: "page-2" };
        if (cursor === "page-2") return { items: [{ id: "b", title: "B duplicate" }, { id: "c", title: "C" }], nextCursor: "page-3" };
        return { items: [{ id: "d", title: "D" }], nextCursor: null, complete: true };
      }
      if (command === "import_remote_playlists") return { state: "completed", completed: 4, total: 4, results: [], errors: [] };
      throw new Error(`Unexpected command ${command}`);
    });
    const target = render(call); await settle();
    target.querySelector<HTMLButtonElement>(".remote-selection-actions button")!.click(); await settle();
    expect(call).toHaveBeenCalledWith("list_remote_playlists", expect.objectContaining({ provider: "spotify", cursor: "page-2", requestId: expect.any(String) }));
    expect(call).toHaveBeenCalledWith("list_remote_playlists", expect.objectContaining({ provider: "spotify", cursor: "page-3", requestId: expect.any(String) }));
    await vi.waitFor(() => expect(target.textContent).toContain("Selected all 4 playlists in this browse."));
    expect(target.textContent).toContain("Selected all 4 playlists in this browse.");
    expect(target.textContent).toContain("4 selected");
    expect(call).not.toHaveBeenCalledWith("import_remote_playlists", expect.anything());
    expect(target.querySelectorAll(".remote-result-row")).toHaveLength(4);
    target.querySelector<HTMLButtonElement>(".remote-import-footer .primary")!.click(); await settle();
    expect(call).toHaveBeenCalledWith("import_remote_playlists", { provider: "spotify", playlistIds: ["a", "b", "c", "d"] });
    target.querySelectorAll<HTMLButtonElement>(".remote-selection-actions button")[1].click(); await settle();
    expect(target.textContent).toContain("0 selected");
  });

  it("imports a multi-page selection as one backend job", async () => {
    const ids = Array.from({ length: 21 }, (_, index) => `playlist-${index + 1}`);
    const call = vi.fn(async (command: string, args?: Record<string, unknown>) => {
      if (command === "list_remote_playlist_bindings") return [];
      if (command === "list_remote_playlists") return { items: ids.map((id, index) => ({ id, title: `Playlist ${index + 1}` })), nextCursor: null };
      if (command === "import_remote_playlists") {
        const playlistIds = args?.playlistIds as string[];
        return { state: "completed", completed: playlistIds.length, total: playlistIds.length, results: playlistIds.map((playlistId, index) => ({ bindingId: `binding-${playlistId}`, collectionId: `collection-${playlistId}`, playlistId, title: `Imported ${index + 1}`, trackCount: 2, complete: true })), errors: [] };
      }
      throw new Error(`Unexpected command ${command}`);
    });
    const target = render(call); await settle();
    target.querySelector<HTMLButtonElement>(".remote-selection-actions button")!.click(); await settle();
    target.querySelector<HTMLButtonElement>(".remote-import-footer .primary")!.click(); await settle();
    const jobs = call.mock.calls.filter(([command]) => command === "import_remote_playlists").map(([, args]) => args?.playlistIds as string[]);
    expect(jobs).toHaveLength(1);
    expect(jobs[0]).toHaveLength(21);
    expect(target.textContent).toContain("21 playlists imported.");
    expect(target.querySelectorAll(".remote-import-results li")).toHaveLength(21);
  });

  it("prefills a direct playlist URL and imports the canonical URL", async () => {
    const url = "https://open.spotify.com/playlist/1234567890123456789012";
    const call = vi.fn(async (command: string) => {
      if (command === "list_remote_playlist_bindings") return [];
      if (command === "import_remote_playlists") return { jobId: "url-job", state: "completed", completed: 1, total: 1, results: [], errors: [] };
      if (command === "get_remote_playlist_import_job") return { jobId: "url-job", state: "completed", completed: 1, total: 1, results: [], errors: [] };
      throw new Error(`Unexpected command ${command}`);
    });
    const target = render(call, { initialPlaylistUrl: url }); await settle();
    expect(target.textContent).toContain(url);
    target.querySelector<HTMLButtonElement>(".remote-import-footer .primary")!.click(); await settle();
    expect(call).toHaveBeenCalledWith("import_remote_playlists", { provider: "spotify", playlistIds: [url] });
  });

  it("stops polling when the picker unmounts and leaves the import running", async () => {
    vi.useFakeTimers();
    const call = vi.fn(async (command: string) => {
      if (command === "list_remote_playlist_bindings") return [];
      if (command === "list_remote_playlists") return { items: [{ id: "mix-a", name: "Late Set" }], nextCursor: null };
      if (command === "import_remote_playlists") return { jobId: "job-2", state: "queued", completed: 0, total: 1, results: [], errors: [] };
      if (command === "get_remote_playlist_import_job") return { jobId: "job-2", state: "running", completed: 0, total: 1, results: [], errors: [] };
      throw new Error(`Unexpected command ${command}`);
    });
    const target = render(call); await settle();
    target.querySelector<HTMLInputElement>('[aria-label="Select Late Set"]')!.click(); await settle();
    target.querySelector<HTMLButtonElement>(".remote-import-footer .primary")!.click(); await settle();
    expect(target.textContent).toContain("Importing 0 of 1");
    destroy?.(); destroy = undefined;
    await vi.advanceTimersByTimeAsync(1000); await settle();
    expect(call).not.toHaveBeenCalledWith("get_remote_playlist_import_job", { jobId: "job-2" });
    expect(call).not.toHaveBeenCalledWith("cancel_remote_playlist_import", { jobId: "job-2" });
  });
});
