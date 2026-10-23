import { describe, expect, it } from "vitest";
import { visibleLyricsTrack } from "./lyrics-request";

describe("visibleLyricsTrack", () => {
  it("passes the reported collaboration title, credits, album, and precise duration", () => {
    expect(visibleLyricsTrack({
      id: "reported-nunca", provider: "youtube", providerId: "reported", sourceUrl: "https://youtube.test/watch?v=reported",
      title: "Nunca vas a comprender", artist: "Rita Payés & Elisabeth Roma", album: "Como la piel", durationSeconds: 244.631,
      favorite: false, provenance: {},
    })).toEqual({
      trackId: "reported-nunca", localPath: null, trackName: "Nunca vas a comprender", artistName: "Rita Payés & Elisabeth Roma", albumName: "Como la piel",
      durationSeconds: 244.631, playbackRange: null,
    });
  });

  it("keeps missing album optional and passes excerpts for backend rejection", () => {
    expect(visibleLyricsTrack({
      id: "excerpt", provider: "youtube", providerId: "excerpt", sourceUrl: "https://youtube.test/watch?v=excerpt",
      title: "Synthetic excerpt", artist: "Synthetic Artist", durationSeconds: 50, playbackRange: { parentTrackId: "parent", chapterId: "chapter", sourceUrl: "source", startTime: 1, endTime: 20, parentTitle: "Synthetic source" },
      favorite: false, provenance: {},
    })).toMatchObject({ albumName: null, durationSeconds: 50, playbackRange: { parentTrackId: "parent" } });
    expect(visibleLyricsTrack(null)).toBeUndefined();
  });
});
