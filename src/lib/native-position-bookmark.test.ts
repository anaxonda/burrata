import { describe, expect, it } from "vitest";
import {
  createNativePositionBookmark,
  nativeBookmarkMatchesOccurrence,
  shouldPersistNativePositionBookmark,
  validNativePositionBookmark,
  type NativePositionCheckpoint,
  type NativePositionOwner,
} from "./native-position-bookmark";

const previous: NativePositionOwner = {
  trackId: "local:a", entryId: "occurrence-a", generation: 3, seekSerial: 4, savedAt: 20_000, paused: false,
};
const checkpoint = (overrides: Partial<NativePositionCheckpoint> = {}): NativePositionCheckpoint => ({
  trackId: "local:a", entryId: "occurrence-a", generation: 3, seekSerial: 4,
  position: 18.5, now: 25_000, paused: false, radio: false, ...overrides,
});

describe("native engine WebView position bookmark policy", () => {
  it("throttles unchanged playing checkpoints to ten seconds", () => {
    expect(shouldPersistNativePositionBookmark(checkpoint(), previous)).toBe(false);
    expect(shouldPersistNativePositionBookmark(checkpoint({ now: 30_000 }), previous)).toBe(true);
  });

  it("forces a pause transition and seek even inside the throttle interval", () => {
    expect(shouldPersistNativePositionBookmark(checkpoint({ paused: true }), previous)).toBe(true);
    expect(shouldPersistNativePositionBookmark(checkpoint({ paused: true, now: 25_001 }), { ...previous, paused: true })).toBe(false);
    expect(shouldPersistNativePositionBookmark(checkpoint({ seekSerial: 5 }), previous)).toBe(true);
  });

  it("checkpoints a new duplicate occurrence and a new native generation", () => {
    expect(shouldPersistNativePositionBookmark(checkpoint({ entryId: "occurrence-b" }), previous)).toBe(true);
    expect(shouldPersistNativePositionBookmark(checkpoint({ generation: 4 }), previous)).toBe(true);
  });

  it("rejects live radio and invalid numeric/owner state", () => {
    expect(shouldPersistNativePositionBookmark(checkpoint({ radio: true }), previous, true)).toBe(false);
    expect(shouldPersistNativePositionBookmark(checkpoint({ position: Infinity }), previous, true)).toBe(false);
    expect(shouldPersistNativePositionBookmark(checkpoint({ generation: -1 }), previous, true)).toBe(false);
  });

  it("guards a stale native bookmark against a different duplicate occurrence while accepting legacy records", () => {
    const bookmark = { trackId: "local:a", entryId: "occurrence-b", position: 18, savedAt: 1 };
    expect(validNativePositionBookmark(bookmark)).toBe(true);
    expect(nativeBookmarkMatchesOccurrence(bookmark, "local:a", "occurrence-b")).toBe(true);
    expect(nativeBookmarkMatchesOccurrence(bookmark, "local:a", "occurrence-a")).toBe(false);
    expect(nativeBookmarkMatchesOccurrence({ trackId: "local:a", position: 18, savedAt: 1 }, "local:a", "occurrence-a")).toBe(true);
  });

  it("carries a newer native cursor into a later WebView-engine restart", () => {
    const staleBookmark = { trackId: "local:a", entryId: "occurrence-a", position: 1101.1, savedAt: 10_000 };
    const resumedNativeCheckpoint = checkpoint({ position: 1718.611, now: 30_000 });
    expect(shouldPersistNativePositionBookmark(resumedNativeCheckpoint, previous)).toBe(true);
    const stored = createNativePositionBookmark(resumedNativeCheckpoint);
    expect(staleBookmark.position).toBeLessThan(stored.position);
    // Simulate cold WebView hydration from the exact localStorage payload.
    const coldStartValue = JSON.parse(JSON.stringify(stored));
    expect(nativeBookmarkMatchesOccurrence(coldStartValue, "local:a", "occurrence-a")).toBe(true);
    expect(coldStartValue.position).toBe(1718.611);
  });

  it("rejects an old same-track bookmark after the selected duplicate queue occurrence changes", () => {
    const oldOccurrence = createNativePositionBookmark(checkpoint({ entryId: "occurrence-a", position: 1718.611 }));
    const restoredQueue = { tracks: ["local:a", "local:a"], entryIds: ["occurrence-a", "occurrence-b"], currentIndex: 1 };
    const selectedTrackId = restoredQueue.tracks[restoredQueue.currentIndex];
    const selectedEntryId = restoredQueue.entryIds[restoredQueue.currentIndex];
    expect(oldOccurrence.trackId).toBe(selectedTrackId);
    expect(nativeBookmarkMatchesOccurrence(oldOccurrence, selectedTrackId, selectedEntryId)).toBe(false);
  });

  it("accepts absolute chapter times supplied by the App integration", () => {
    const absoluteChapterPosition = 93.25;
    expect(shouldPersistNativePositionBookmark(checkpoint({ position: absoluteChapterPosition, paused: true }), previous)).toBe(true);
  });
});
