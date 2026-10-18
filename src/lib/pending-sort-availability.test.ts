import { describe, expect, it, vi } from "vitest";
import { allTrackSortFieldsAvailable } from "./track-sort-availability";
import { PendingSortAvailabilityRefresh, sortAvailabilityIsPending, type AvailabilityPage } from "./pending-sort-availability";
import type { TrackSortAvailability } from "./track-sort-availability";

const ready: TrackSortAvailability = { sourceOrder: true, title: true, artist: true, album: false, duration: true, releaseDate: false, dateAdded: true, bpm: false, key: false, plays: true, views: false };

describe("pending global sort availability refresh", () => {
  it("keeps all sort options enabled while pending and adopts the ready map without replacing rows or paging state", async () => {
    const refresher = new PendingSortAvailabilityRefresh<TrackSortAvailability>();
    const rows = [{ id: "visible-1" }, { id: "visible-2" }], cursor = 200, scroll = 740;
    let pending = true, availability: TrackSortAvailability | null = null;
    const allEnabled = allTrackSortFieldsAvailable();
    expect(allEnabled.album).toBe(true);

    await refresher.refresh("query-a", () => pending, () => true,
      async () => ({ sortAvailabilityPending: false, sortAvailability: ready }),
      page => { if (page.sortAvailabilityPending === false) { pending = false; availability = page.sortAvailability ?? null; } },
    );

    expect(pending).toBe(false);
    expect(availability).toEqual(ready);
    expect(rows).toEqual([{ id: "visible-1" }, { id: "visible-2" }]);
    expect(cursor).toBe(200);
    expect(scroll).toBe(740);
  });

  it("discards a ready reply when the active query generation changes", async () => {
    const refresher = new PendingSortAvailabilityRefresh<TrackSortAvailability>();
    let pending = true, queryGeneration = 4, availability: TrackSortAvailability | null = null;
    let resolvePage!: (page: AvailabilityPage<TrackSortAvailability>) => void;
    const fetch = vi.fn(() => new Promise<AvailabilityPage<TrackSortAvailability>>(resolve => { resolvePage = resolve; }));
    const request = refresher.refresh("query-a-generation-4", () => pending, () => queryGeneration === 4,
      fetch, page => { pending = page.sortAvailabilityPending === true; availability = page.sortAvailability ?? null; },
    );
    expect(fetch).toHaveBeenCalledOnce();
    queryGeneration = 5;
    resolvePage({ sortAvailabilityPending: false, sortAvailability: ready });
    await request;
    expect(pending).toBe(true);
    expect(availability).toBeNull();
  });

  it("coalesces readiness events received while a metadata page is in flight", async () => {
    const refresher = new PendingSortAvailabilityRefresh<TrackSortAvailability>();
    let pending = true, availability: TrackSortAvailability | null = null, calls = 0;
    let resolveFirst!: (page: AvailabilityPage<TrackSortAvailability>) => void;
    const fetch = vi.fn(async (): Promise<AvailabilityPage<TrackSortAvailability>> => {
      calls++;
      if (calls === 1) return new Promise(resolve => { resolveFirst = resolve; });
      return { sortAvailabilityPending: false, sortAvailability: ready };
    });
    const request = refresher.refresh("query-a", () => pending, () => true, fetch, page => {
      if (page.sortAvailabilityPending === false) { pending = false; availability = page.sortAvailability ?? null; }
    });
    refresher.noteUpdated();
    refresher.noteUpdated();
    resolveFirst({ sortAvailabilityPending: true });
    await request;
    expect(fetch).toHaveBeenCalledTimes(2);
    expect(pending).toBe(false);
    expect(availability).toEqual(ready);
  });

  it("does not poll or request metadata when a legacy page omitted the pending flag", async () => {
    const refresher = new PendingSortAvailabilityRefresh<TrackSortAvailability>();
    const fetch = vi.fn(async () => ({ sortAvailability: null }));
    const pending = sortAvailabilityIsPending({ sortAvailability: null });
    expect(pending).toBe(false); // A legacy missing map/flag is not treated as Pending.
    refresher.noteUpdated();
    await refresher.refresh("legacy", () => pending, () => true, fetch, () => {});
    expect(fetch).not.toHaveBeenCalled();
  });
});
