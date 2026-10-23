import { describe, expect, it } from "vitest";
import { isCurrentLiveBpmEstimate } from "./live-bpm";
import type { LiveBpmEstimate } from "./types";

const owner = { generation: 8, seekSerial: 3, trackId: "song-a" };
const estimate: LiveBpmEstimate = { ...owner, bpm: 123.4, confidence: 0.82, relatedTempoAlternativeBpm: 61.7, windowSeconds: 8 };

describe("live BPM response ownership", () => {
  it("accepts only a finite native estimate for the current generation, seek, and track", () => {
    expect(isCurrentLiveBpmEstimate(estimate, owner)).toBe(true);
    expect(isCurrentLiveBpmEstimate({ ...estimate, generation: 7 }, owner)).toBe(false);
    expect(isCurrentLiveBpmEstimate({ ...estimate, seekSerial: 2 }, owner)).toBe(false);
    expect(isCurrentLiveBpmEstimate({ ...estimate, trackId: "song-b" }, owner)).toBe(false);
    expect(isCurrentLiveBpmEstimate({ ...estimate, bpm: Number.NaN }, owner)).toBe(false);
    expect(isCurrentLiveBpmEstimate({ ...estimate, confidence: 1.2 }, owner)).toBe(false);
  });
});
