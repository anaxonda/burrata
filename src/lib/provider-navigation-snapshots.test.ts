import { describe, expect, it } from "vitest";
import { ProviderNavigationSnapshots, providerAccountScope, type ProviderSnapshotEntry } from "./provider-navigation-snapshots";

type Snapshot = { controls: { filter: string }; rows: readonly { id: number }[]; loadedCount: number };
const entry = (key: string, accountScope: string, rows: readonly { id: number }[]): ProviderSnapshotEntry<Snapshot> => ({
  kind: "playlist-detail", key, accountScope, snapshot: { controls: { filter: key }, rows, loadedCount: rows.length }, rows: rows.length,
  stripRows: value => ({ ...value, rows: [] }),
});

describe("ProviderNavigationSnapshots", () => {
  it("bounds entries and total retained rows while preserving controls on row eviction", () => {
    const cache = new ProviderNavigationSnapshots(2, 3);
    const rowsA = [{ id: 1 }, { id: 2 }];
    cache.put(entry("a", "spotify:revision:1", rowsA));
    cache.put(entry("b", "spotify:revision:1", [{ id: 3 }, { id: 4 }]));
    expect(cache.rows).toBeLessThanOrEqual(3);
    const retained = cache.get<Snapshot>("playlist-detail", "a", "spotify:revision:1");
    expect(retained?.controls.filter).toBe("a");
    expect(retained?.loadedCount).toBe(2);
    expect(retained?.rows.length).toBe(0);
    cache.put(entry("c", "spotify:revision:1", [{ id: 5 }]));
    expect(cache.size).toBe(2);
  });

  it("requires an exact account scope and invalidates only the requested provider kind", () => {
    const cache = new ProviderNavigationSnapshots();
    cache.put(entry("mix", "spotify:revision:4", [{ id: 1 }]));
    cache.put({ ...entry("artist", "spotify:revision:4", [{ id: 2 }]), kind: "spotify-artist" });
    const stale = cache.get<Snapshot>("playlist-detail", "mix", "spotify:revision:5");
    expect(stale?.controls.filter).toBe("mix");
    expect(stale?.rows).toHaveLength(0);
    cache.invalidateAccount("playlist-detail", "spotify:revision:4");
    const invalidated = cache.get<Snapshot>("playlist-detail", "mix", "spotify:revision:4");
    expect(invalidated?.rows).toHaveLength(0);
    expect(cache.get("spotify-artist", "artist", "spotify:revision:4")).not.toBeNull();
  });

  it("retains row array references rather than cloning provider rows", () => {
    const cache = new ProviderNavigationSnapshots();
    const rows = [{ id: 1 }];
    cache.put(entry("same", "spotify:revision:1", rows));
    expect(cache.get<Snapshot>("playlist-detail", "same", "spotify:revision:1")?.rows).toBe(rows);
  });

  it("uses an existing managed revision and refuses unknown or reconnect-required account scopes", async () => {
    const managed = async <T = unknown>(command: string): Promise<T> => {
      if (command === "get_managed_provider_session_status") return { accountRevisions: { spotify: 3 }, spotifyAccountId: "account-a" } as T;
      throw new Error(command);
    };
    expect(await providerAccountScope(managed, "spotify")).toBe("spotify:revision:3:account-a");
    let fallbackCalls = 0;
    const stopped = async <T = unknown>(command: string): Promise<T> => {
      if (command === "get_managed_provider_session_status") return { enabled: true, running: false, mode: "stopped", spotifyAccountId: "old-account", accountRevisions: { spotify: 8 } } as T;
      if (command === "remote_playlist_connection_status") { fallbackCalls += 1; return { connected: true, accountId: "old-account" } as T; }
      throw new Error(command);
    };
    expect(await providerAccountScope(stopped, "spotify")).toBeNull();
    expect(fallbackCalls).toBe(0);
    const nativeConfirmed = async <T = unknown>(command: string): Promise<T> => {
      if (command === "get_managed_provider_session_status") return { enabled: true, running: true, mode: "native-confirmed", spotifyAccountId: "old-account", accountRevisions: { spotify: 8 } } as T;
      throw new Error(command);
    };
    expect(await providerAccountScope(nativeConfirmed, "spotify")).toBeNull();
    const unknown = async <T = unknown>(command: string): Promise<T> => {
      if (command === "get_managed_provider_session_status") return { youtubeAccountId: "old-channel", youtubeReconnectRequired: true, accountRevisions: { youtube: 9 } } as T;
      throw new Error(command);
    };
    expect(await providerAccountScope(unknown, "youtube")).toBeNull();
    const fallback = async <T = unknown>(command: string): Promise<T> => {
      if (command === "get_managed_provider_session_status") throw new Error("unsupported");
      if (command === "remote_playlist_connection_status") return { connected: false, accountId: "stale-account" } as T;
      throw new Error(command);
    };
    expect(await providerAccountScope(fallback, "spotify")).toBeNull();
  });
});


describe("catalog artist history eviction",()=>{
  it("retains immutable artist controls after the default twenty-entry row cache evicts the page",()=>{
    const cache=new ProviderNavigationSnapshots();
    expect(cache.maxEntries).toBe(20);
    const snapshot={query:"previous-filter",items:["source-row"],loadedCount:1,needsRefetch:false};
    cache.put({kind:"artist-page",key:"previous-entry",accountScope:"public-catalog",snapshot,rows:1,stripRows:value=>({...value,items:[],needsRefetch:true})});
    for(let index=0;index<20;index++)cache.put({kind:"artist-page",key:`new-${index}`,accountScope:"public-catalog",snapshot:{query:"new-filter",items:[],loadedCount:0,needsRefetch:false},rows:0,stripRows:value=>({...value,items:[],needsRefetch:true})});
    expect(cache.size).toBe(20);
    expect(cache.get("artist-page","previous-entry","public-catalog")).toEqual({query:"previous-filter",items:[],loadedCount:1,needsRefetch:true});
    expect(snapshot).toEqual({query:"previous-filter",items:["source-row"],loadedCount:1,needsRefetch:false});
  });
});
