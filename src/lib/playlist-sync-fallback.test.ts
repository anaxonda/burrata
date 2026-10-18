import { describe, expect, it, vi } from "vitest";
import { runPeriodicPlaylistSyncFallback } from "./playlist-sync-fallback";

describe("periodic playlist sync fallback", () => {
  it("skips duplicate sync when the native watcher is healthy", async () => {
    const sync = vi.fn();
    const ran = await runPeriodicPlaylistSyncFallback(async () => true, sync);
    expect(ran).toBe(false);
    expect(sync).not.toHaveBeenCalled();
  });

  it.each([
    ["watcher unavailable", async () => false],
    ["health check failed", async () => { throw new Error("unavailable"); }],
  ])("runs fallback when %s", async (_label, watcherHealthy) => {
    const sync = vi.fn();
    const ran = await runPeriodicPlaylistSyncFallback(watcherHealthy, sync);
    expect(ran).toBe(true);
    expect(sync).toHaveBeenCalledOnce();
  });
});
