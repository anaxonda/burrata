import { describe, expect, it, vi } from "vitest";
import { acquireVirtualScrollRoot, activateVirtualScroll, afterVirtualTick, getSavedVirtualHeights, getSavedVirtualScroll, prefixHeights, releaseVirtualScrollRoot, scrollAnchorDelta, setSavedVirtualHeights, setSavedVirtualScroll, virtualWindow } from "./virtual-results";

describe("variable-height virtual result windows", () => {
  it("bounds a large mixed-height viewport and calculates spacer sizes", () => {
    const heights = Array.from({ length: 1_200 }, (_, index) => index % 4 === 0 ? 96 : index % 3 === 0 ? 54 : 66);
    const prefix = prefixHeights(heights.length, heights, () => 64);
    const window = virtualWindow(prefix, prefix[800]!, 600, 6);
    expect(window.start).toBeLessThanOrEqual(800);
    expect(window.end).toBeGreaterThan(800);
    expect(window.end - window.start).toBeLessThan(30);
    expect(window.top).toBe(prefix[window.start]);
    expect(window.bottom).toBe(prefix[heights.length]! - prefix[window.end]!);
  });

  it("supports an empty result list and clamps scrolling beyond the end", () => {
    expect(virtualWindow([0], 0, 500)).toEqual({ start: 0, end: 0, top: 0, bottom: 0 });
    expect(virtualWindow([0, 60, 120], 100_000, 400, 0).start).toBe(0);
  });

  it("keeps the row containing a partial scroll offset in a zero-overscan window", () => {
    expect(virtualWindow([0, 60, 120, 180], 30, 1, 0)).toEqual({ start: 0, end: 1, top: 0, bottom: 120 });
  });

  it("keeps the visible anchor still when an earlier expanded row changes height", () => {
    expect(scrollAnchorDelta(2, 6, 60, 154)).toBe(94);
    expect(scrollAnchorDelta(6, 6, 60, 154)).toBe(0);
  });

  it("retains scroll positions by search and sort key", () => {
    setSavedVirtualScroll("query:title", 12_345);
    expect(getSavedVirtualScroll("query:title")).toBe(12_345);
    expect(getSavedVirtualScroll("query:artist")).toBe(0);
  });

  it("restores measured heights by view and source region at the same width", () => {
    const measured = new Map([["source:video:1", 137], ["source:video:2", 82]]);
    setSavedVirtualHeights("back-view", "source", 900, measured);
    setSavedVirtualHeights("back-view", "discogs", 900, new Map([["discogs:artist:1", 111]]));
    setSavedVirtualHeights("back-view", "source", 900, new Map([["source:video:3", 94]]));
    const restored = getSavedVirtualHeights("back-view", "source", 900);
    expect(restored).toEqual(new Map([...measured, ["source:video:3", 94]]));
    expect(getSavedVirtualHeights("back-view", "discogs", 900).get("discogs:artist:1")).toBe(111);
    expect(prefixHeights(2, [restored.get("source:video:1")!, restored.get("source:video:2")!], () => 76)).toEqual([0, 137, 219]);
    expect(getSavedVirtualHeights("back-view", "source", 1_100).size).toBe(0);
    const root = {} as HTMLElement;
    setSavedVirtualScroll("back-view", 1_246);
    expect(activateVirtualScroll(root, "away-view", 1_246)).toBe(0);
    expect(activateVirtualScroll(root, "back-view", 0)).toBe(1_246);
    expect(getSavedVirtualHeights("back-view", "source", 900).get("source:video:1")).toBe(137);
  });

  it("preserves a scrolled root when a late source region attaches to the same view", () => {
    const root = {} as HTMLElement;
    const initialRegion = acquireVirtualScrollRoot(root);
    expect(activateVirtualScroll(root, "search:first", 0)).toBe(0);

    // The first region scrolls the shared panel; attaching another region must not
    // restore the stale saved position over the user's current position.
    const lateRegion = acquireVirtualScrollRoot(root);
    expect(activateVirtualScroll(root, "search:first", 840)).toBeNull();
    releaseVirtualScrollRoot(lateRegion, "search:first", 840);
    expect(activateVirtualScroll(root, "search:first", 840)).toBeNull();
    releaseVirtualScrollRoot(initialRegion, "search:first", 840);
    expect(getSavedVirtualScroll("search:first")).toBe(840);
  });

  it("restores a remounted view and changes all regions together across query keys", () => {
    const root = {} as HTMLElement;
    const region = acquireVirtualScrollRoot(root);
    expect(activateVirtualScroll(root, "query:a", 0)).toBe(0);
    releaseVirtualScrollRoot(region, "query:a", 510);

    // Empty results can unmount every region while leaving the panel DOM in place.
    // Returning to the same view key on that root must restore its saved position.
    const returnedRegion = acquireVirtualScrollRoot(root);
    expect(activateVirtualScroll(root, "query:a", 0)).toBe(510);
    const secondRegion = acquireVirtualScrollRoot(root);
    expect(activateVirtualScroll(root, "query:a", 510)).toBeNull();

    // One query change updates the shared root; the other region joins without
    // resetting it. Removing one region must leave the binding alive.
    expect(activateVirtualScroll(root, "query:b", 510)).toBe(0);
    releaseVirtualScrollRoot(secondRegion, "query:b", 275);
    releaseVirtualScrollRoot(secondRegion, "query:b", 275); // cleanup can run twice
    expect(activateVirtualScroll(root, "query:b", 275)).toBeNull();
    releaseVirtualScrollRoot(returnedRegion, "query:b", 275);
    expect(activateVirtualScroll(root, "query:a", 275)).toBe(510);

    // A fresh root represents navigation back after the previous panel was torn down.
    const remountedRoot = {} as HTMLElement;
    acquireVirtualScrollRoot(remountedRoot);
    expect(activateVirtualScroll(remountedRoot, "query:a", 0)).toBe(510);
  });

  it("does not attach viewport listeners when unmounted before the initial tick", async () => {
    let resume!: () => void;
    const tick = new Promise<void>(resolve => { resume = resolve; });
    let disposed = false;
    const attach = vi.fn();
    const pending = afterVirtualTick(() => tick, () => disposed, attach);
    disposed = true;
    resume();
    await pending;
    expect(attach).not.toHaveBeenCalled();
  });
});
