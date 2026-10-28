import { describe, expect, it } from "vitest";
import { cropWaveformBins, emptyWaveform, localWaveformSnapshot, observeWaveformSegment, streamWaveformSnapshot, waveformCoverage, waveformStatus } from "./waveform-data";

describe("observed waveform data", () => {
  it("crops a source-time chapter to a zero-based playback seek axis", () => {
    const bins = Array.from({ length: 600 }, (_, index) => ({ min: index / 600, max: (index + 1) / 600 }));
    const chapter = cropWaveformBins({ bins, durationSeconds: 600, startSeconds: 300, endSeconds: 310, outputBinCount: 100 });
    expect(chapter).toHaveLength(100);
    expect(chapter[0]?.min).toBeCloseTo(0.5, 2);
    expect(chapter[0]?.max).toBeGreaterThan(chapter[0]!.min);
    expect(chapter[99]?.max).toBeCloseTo(310 / 600, 2);
  });

  it("leaves unheard time blank and merges only bins touched by current-generation observations", () => {
    let snapshot = emptyWaveform(4, "track-a", 100, 10);
    snapshot = observeWaveformSegment(snapshot, { generation: 3, trackId: "track-a", startSeconds: 0, endSeconds: 10, min: -0.5, max: 0.4 });
    expect(snapshot.bins.every(bin => bin === null)).toBe(true);
    snapshot = observeWaveformSegment(snapshot, { generation: 4, trackId: "track-b", startSeconds: 30, endSeconds: 40, min: -0.5, max: 0.4 });
    expect(snapshot.bins.every(bin => bin === null)).toBe(true);
    snapshot = observeWaveformSegment(snapshot, { generation: 4, trackId: "track-a", startSeconds: 20, endSeconds: 30, min: -0.6, max: 0.3 });
    snapshot = observeWaveformSegment(snapshot, { generation: 4, trackId: "track-a", startSeconds: 21, endSeconds: 29, min: -0.2, max: 0.8 });
    expect(snapshot.bins).toEqual([null, null, { min: -0.6, max: 0.8 }, null, null, null, null, null, null, null]);
    expect(waveformCoverage(snapshot)).toEqual({ observedBins: 1, totalBins: 10, fraction: 0.1 });
    expect(waveformStatus(snapshot)).toContain("Observed playback only");
    expect(waveformStatus(snapshot)).toContain("blank areas have not been analyzed");
  });

  it("clamps valid timed segments to the resource duration and ignores invalid values", () => {
    let snapshot = emptyWaveform(8, "track", 10, 10);
    snapshot = observeWaveformSegment(snapshot, { generation: 8, trackId: "track", startSeconds: 8, endSeconds: 14, min: -1, max: 1 });
    snapshot = observeWaveformSegment(snapshot, { generation: 8, trackId: "track", startSeconds: 2, endSeconds: 4, min: -2, max: 1 });
    expect(snapshot.bins.slice(8)).toEqual([{ min: -1, max: 1 }, { min: -1, max: 1 }]);
    expect(snapshot.bins.slice(0, 8).every(bin => bin === null)).toBe(true);
  });

  it("only labels a fully covered local analysis as a full waveform", () => {
    const partial = localWaveformSnapshot({ generation: 1, trackId: "local", durationSeconds: 3, bins: [{ min: -0.2, max: 0.1 }, null, { min: -0.6, max: 0.4 }], complete: false });
    expect(partial.complete).toBe(false);
    expect(waveformStatus(partial)).not.toContain("Full audio waveform");
    expect(() => localWaveformSnapshot({ generation: 1, trackId: "local", durationSeconds: 3, bins: [{ min: 0, max: 0.2 }, null], complete: true })).toThrow(/cover the entire/);
    const full = localWaveformSnapshot({ generation: 1, trackId: "local", durationSeconds: 3, bins: [{ min: -0.2, max: 0.1 }, { min: -0.6, max: 0.4 }], complete: true });
    expect(waveformStatus(full)).toBe("Full audio waveform");
  });

  it("labels complete tap coverage as observed playback rather than local analysis", () => {
    let snapshot = emptyWaveform(2, "track", 1, 2);
    snapshot = observeWaveformSegment(snapshot, { generation: 2, trackId: "track", startSeconds: 0, endSeconds: 0.5, min: -0.4, max: 0.5 });
    snapshot = observeWaveformSegment(snapshot, { generation: 2, trackId: "track", startSeconds: 0.5, endSeconds: 1, min: -0.5, max: 0.4 });
    snapshot = { ...snapshot, complete: true };
    expect(waveformStatus(snapshot)).toContain("Complete playback observed");
    expect(waveformStatus(snapshot)).not.toBe("Full audio waveform");
  });

  it("installs only a complete stream waveform bound to its stable provider resource", () => {
    const bins = Array.from({ length: 512 }, (_, index) => ({ min: -index / 512, max: index / 512 }));
    const snapshot = streamWaveformSnapshot({
      generation: 19, trackId: "recording-1", provider: "youtube", sourceId: "abc123",
      sourceIdentitySha256: "a".repeat(64), durationSeconds: 210, bins, complete: true, scope: "wholeTrack",
    });
    expect(snapshot.source).toBe("stream-analysis");
    expect(snapshot.complete).toBe(true);
    expect(snapshot.resource).toEqual({ provider: "youtube", sourceId: "abc123", sourceIdentitySha256: "a".repeat(64) });
    expect(waveformStatus(snapshot)).toBe("Full verified stream waveform");
    expect(() => streamWaveformSnapshot({
      generation: 19, trackId: "recording-1", provider: "youtube", sourceId: "abc123",
      sourceIdentitySha256: "a".repeat(64), durationSeconds: 210, bins: [null, ...bins.slice(1)], complete: true, scope: "wholeTrack",
    })).toThrow(/complete/);
    expect(() => streamWaveformSnapshot({
      generation: 19, trackId: "recording-1", provider: "youtube", sourceId: "abc123",
      sourceIdentitySha256: "a".repeat(63), durationSeconds: 210, bins, complete: true, scope: "wholeTrack",
    })).toThrow(/identity/);
  });

  it("bounds bin counts and validates identifiers and amplitudes", () => {
    expect(() => emptyWaveform(1, "track", 10, 100_000)).toThrow(/bin count/);
    expect(() => emptyWaveform(-1, "track", 10, 10)).toThrow(/generation/);
    expect(() => localWaveformSnapshot({ generation: 1, trackId: "track", durationSeconds: 1, bins: [{ min: -0.1, max: 1.1 }], complete: false })).toThrow(/amplitude/);
  });
});
