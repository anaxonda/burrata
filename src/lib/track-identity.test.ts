import { describe, expect, it } from "vitest";
import { sameTrackIdentity, trackIdentity } from "./track-identity";
import type { Track } from "./types";

const recording = (id: string, overrides: Partial<Track> = {}): Track => ({
  id, provider: "library", providerId: id, sourceUrl: "https://example.test/recording",
  artist: "Artist", title: "Recording", favorite: false, provenance: {}, ...overrides,
});

const chapter = (chapterId: string, overrides: Partial<Track["playbackRange"]> = {}) => recording("chapter:recording-1", {
  title: `Chapter ${chapterId}`,
  playbackRange: {
    parentTrackId: "recording-1", chapterId, sourceUrl: "https://example.test/mix",
    startTime: 30, endTime: 90, parentTitle: "Full mix", ...overrides,
  },
});

describe("track identity", () => {
  it("matches the same canonical recording across different list representations", () => {
    expect(sameTrackIdentity(recording("recording-1"), recording("recording-1", {
      provider: "spotify", providerId: "spotify:asset", sourceUrl: "https://spotify.test/track",
      title: "Updated title", favorite: true,
    }))).toBe(true);
    expect(sameTrackIdentity(recording("recording-1"), recording("recording-2"))).toBe(false);
  });

  it("distinguishes full recordings from chapters and identifies chapters by their source range", () => {
    const first = chapter("one");
    expect(trackIdentity(first)).toContain("chapter");
    expect(sameTrackIdentity(first, chapter("one"))).toBe(true);
    expect(sameTrackIdentity(first, recording("recording-1"))).toBe(false);
    expect(sameTrackIdentity(first, chapter("two"))).toBe(false);
    expect(sameTrackIdentity(first, chapter("one", { startTime: 31 }))).toBe(false);
    expect(sameTrackIdentity(first, chapter("one", { sourceUrl: "https://example.test/other-mix" }))).toBe(false);
  });
});
