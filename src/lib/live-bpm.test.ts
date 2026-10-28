import { describe, expect, it } from "vitest";
import { isCurrentLiveBpmEstimate, isCurrentLiveKeyEstimate, liveBpmLabel } from "./live-bpm";
import type { LiveBpmEstimate, LiveKeyEstimate } from "./types";

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

describe("live key response ownership", () => {
  const keyEstimate: LiveKeyEstimate = { ...owner, key: "Am", profileFit: 0.34, profileMargin: 0.08, windowSeconds: 12 };
  it("accepts only a qualified key for the exact generation, seek and track", () => {
    expect(isCurrentLiveKeyEstimate(keyEstimate, owner)).toBe(true);
    expect(isCurrentLiveKeyEstimate({ ...keyEstimate, generation: 7 }, owner)).toBe(false);
    expect(isCurrentLiveKeyEstimate({ ...keyEstimate, seekSerial: 2 }, owner)).toBe(false);
    expect(isCurrentLiveKeyEstimate({ ...keyEstimate, trackId: "song-b" }, owner)).toBe(false);
    expect(isCurrentLiveKeyEstimate({ ...keyEstimate, key: "H" }, owner)).toBe(false);
    expect(isCurrentLiveKeyEstimate({ ...keyEstimate, profileFit: 0.1 }, owner)).toBe(false);
    expect(isCurrentLiveKeyEstimate({ ...keyEstimate, profileMargin: Number.NaN }, owner)).toBe(false);
    expect(isCurrentLiveKeyEstimate({ ...keyEstimate, windowSeconds: 13 }, owner)).toBe(false);
  });
});

describe("live BPM presentation", () => {
  it("shows supported octave alternatives in ascending order without a strength percentage", () => {
    expect(liveBpmLabel({bpm:172,relatedTempoAlternativeBpm:86})).toBe("86 / 172 BPM");
    expect(liveBpmLabel({bpm:86,relatedTempoAlternativeBpm:172})).toBe("86 / 172 BPM");
    expect(liveBpmLabel({bpm:84.6,relatedTempoAlternativeBpm:null})).toBe("84.6 BPM");
    expect(liveBpmLabel({bpm:86,relatedTempoAlternativeBpm:NaN})).toBe("86 BPM");
    expect(liveBpmLabel({bpm:86,relatedTempoAlternativeBpm:92})).toBe("86 BPM");
  });
});
