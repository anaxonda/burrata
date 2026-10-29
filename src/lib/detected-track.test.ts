import { describe, expect, it } from "vitest";
import { detectedObservationTrack } from "./detected-track";
import type { DetectedTrackObservation } from "./playing-track-detection";

function observation(overrides: Partial<DetectedTrackObservation> = {}): DetectedTrackObservation {
  return {
    title: "Recognized song",
    artist: "Recognized artist",
    detectedAtSeconds: 124,
    capturedStartSeconds: 120,
    capturedEndSeconds: 128,
    observedAt: "2026-10-28T12:00:00Z",
    ...overrides,
  };
}

describe("detectedObservationTrack", () => {
  it("uses stable recognition identity and keeps usable metadata without inheriting parent playback state", () => {
    const first = detectedObservationTrack(observation({
      title: "Recognized song", identifiers: { isrc: "usabc2600001", shazamId: "42" }, artworkUrl: "https://images.example.test/cover.jpg",
    }));
    const refreshed = detectedObservationTrack(observation({
      title: "Corrected title", identifiers: { isrc: "USABC2600001", shazamId: "42" },
    }));

    expect(first.id).toBe(refreshed.id);
    expect(first).toMatchObject({
      id: "detected:isrc%3AUSABC2600001",
      provider: "detected",
      providerId: "isrc%3AUSABC2600001",
      title: "Recognized song",
      artist: "Recognized artist",
      sourceUrl: "",
      favorite: false,
      artworkUrl: "https://images.example.test/cover.jpg",
    });
    expect(first.playbackRange).toBeUndefined();
    expect(first.provenance).toMatchObject({ sourceKind: "detected-track", shazamId: "42", detectedAtSeconds: 124 });
  });

  it("falls back to a stable normalized artist and title identity", () => {
    const first = detectedObservationTrack(observation());
    const refreshed = detectedObservationTrack(observation({ title: "RECOGNIZED SONG", artist: " recognized artist " }));
    expect(first.id).toBe(refreshed.id);
    expect(first.providerId).toContain("recognized%20artist");
  });
});
