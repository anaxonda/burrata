import { describe, expect, it } from "vitest";
import { approximateDetectionTime, playingTrackDetectionOwnerKey, samePlayingTrackDetectionOwner } from "./playing-track-detection";

describe("playing track detection presentation and ownership", () => {
  it("fences stale replies across recording, source generation, and seek changes", () => {
    const owner = { trackId: "mix-1", generation: 4, seekSerial: 2 };
    expect(samePlayingTrackDetectionOwner(owner, { ...owner })).toBe(true);
    expect(samePlayingTrackDetectionOwner(owner, { ...owner, trackId: "mix-2" })).toBe(false);
    expect(samePlayingTrackDetectionOwner(owner, { ...owner, generation: 5 })).toBe(false);
    expect(samePlayingTrackDetectionOwner(owner, { ...owner, seekSerial: 3 })).toBe(false);
    expect(samePlayingTrackDetectionOwner(owner, null)).toBe(false);
  });

  it("changes its owner key on every source or seek transition", () => {
    expect(playingTrackDetectionOwnerKey({ trackId: "mix", generation: 1, seekSerial: 1 }))
      .not.toBe(playingTrackDetectionOwnerKey({ trackId: "mix", generation: 2, seekSerial: 1 }));
    expect(playingTrackDetectionOwnerKey({ trackId: "mix", generation: 1, seekSerial: 1 }))
      .not.toBe(playingTrackDetectionOwnerKey({ trackId: "mix", generation: 1, seekSerial: 2 }));
    expect(playingTrackDetectionOwnerKey(null)).toBeNull();
  });

  it("labels markers as approximate source positions", () => {
    expect(approximateDetectionTime(125.8)).toBe("~2:05");
    expect(approximateDetectionTime(3725)).toBe("~1:02:05");
    expect(approximateDetectionTime(Number.NaN)).toBe("time unavailable");
  });
});
