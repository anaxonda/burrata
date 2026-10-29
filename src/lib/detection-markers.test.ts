import { describe, expect, it } from "vitest";
import { detectionMarkers } from "./detection-markers";
import type { DetectedTrackObservation } from "./playing-track-detection";

const observation = (start: number, end: number, lastConfirmedAtSeconds?: number): DetectedTrackObservation => ({
  title: "Song", artist: "Artist", detectedAtSeconds: (start + end) / 2,
  ...(lastConfirmedAtSeconds === undefined ? {} : { lastConfirmedAtSeconds }),
  capturedStartSeconds: start, capturedEndSeconds: end, observedAt: "2026-01-01T00:00:00Z",
});

describe("detection markers", () => {
  it("clamps valid approximate windows to the displayed timeline", () => {
    expect(detectionMarkers([observation(-3, 12), observation(95, 110)], 100)).toMatchObject([
      { startSeconds: 0, endSeconds: 12 },
      { startSeconds: 95, endSeconds: 100 },
    ]);
  });

  it("ignores invalid ranges and durations", () => {
    expect(detectionMarkers([observation(10, 10), observation(80, 120)], 100)).toHaveLength(1);
    expect(detectionMarkers([observation(0, 1)], 0)).toEqual([]);
  });

  it("uses the same color for repeated detections of the same song", () => {
    const [first, second] = detectionMarkers([observation(10, 20), observation(50, 60)], 100);
    expect(first?.colorIndex).toBe(second?.colorIndex);
  });

  it("extends the marker through the latest confirmed sample only", () => {
    expect(detectionMarkers([observation(10, 22, 80)], 100)).toMatchObject([
      { startSeconds: 10, endSeconds: 86 },
    ]);
  });

  it("maps absolute detection times onto a range-relative scrubber", () => {
    expect(detectionMarkers([observation(120, 132, 150)], 60, 120)).toMatchObject([
      { startSeconds: 0, endSeconds: 36 },
    ]);
    expect(detectionMarkers([observation(175, 190)], 60, 120)).toMatchObject([
      { startSeconds: 55, endSeconds: 60 },
    ]);
  });
});
