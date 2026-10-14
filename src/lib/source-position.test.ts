import { describe, expect, it } from "vitest";
import { playlistPosition, sourcePosition } from "./source-position";
import type { Track } from "./types";
const track = (rank: number | null, provenance: unknown) => ({ rank, provenance }) as Track;
describe("source position", () => {
  it("retains a chart rank independently of playlist provenance", () => {
    expect(sourcePosition(track(12, { playlistPosition: 3 }))).toBe(12);
  });
  it("shows playlist order without claiming a chart rank", () => {
    const item = track(null, { playlistPosition: 8 });
    expect(sourcePosition(item)).toBe(8);
    expect(item.rank).toBeNull();
  });
  it("leaves invalid and missing positions unknown", () => {
    for (const value of [0, -1, 2.5, "2", null, Infinity]) {
      expect(playlistPosition(track(null, { playlistPosition: value }))).toBeNull();
    }
    expect(sourcePosition(track(null, null))).toBeNull();
  });
});
