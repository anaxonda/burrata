// @vitest-environment jsdom
import { describe, expect, it } from "vitest";
import { emptyWaveform, observeWaveformSegment } from "./waveform-data";
import { isFreshWebviewWaveformWindowAfterSeek, type WebviewWaveformSeekDrain } from "./webview-waveform-seek";

describe("WebView waveform seek drain", () => {
  it("keeps the target bin unobserved until a complete post-seek analyser window arrives", () => {
    const node = document.createElement("audio");
    Object.defineProperties(node, {
      seeking: { configurable: true, value: false },
      readyState: { configurable: true, value: 4 },
    });
    const target = 42;
    const drain: WebviewWaveformSeekDrain = { node, sourceGeneration: 12, seekSerial: 7, targetPositionSeconds: target };
    const initial = emptyWaveform(3, "track", 120, 1200);
    const staleRingWindow = { startPositionSeconds: target - 0.02, endPositionSeconds: target + 0.022 };
    expect(isFreshWebviewWaveformWindowAfterSeek(node, 12, 7, staleRingWindow, drain)).toBe(false);
    const unchanged = initial;
    expect(unchanged.bins[420]).toBeNull();

    const freshWindow = { startPositionSeconds: target, endPositionSeconds: target + 0.043 };
    expect(isFreshWebviewWaveformWindowAfterSeek(node, 12, 7, freshWindow, drain)).toBe(true);
    const observed = observeWaveformSegment(initial, {
      generation: 3, trackId: "track", startSeconds: freshWindow.startPositionSeconds,
      endSeconds: freshWindow.endPositionSeconds, min: -0.4, max: 0.6,
    });
    expect(observed.bins[420]).toMatchObject({ min: -0.4, max: 0.6 });
  });

  it("requires settled media and applies the drain only to its owning seek", () => {
    const node = document.createElement("audio");
    const window = { startPositionSeconds: 10, endPositionSeconds: 10.05 };
    const drain: WebviewWaveformSeekDrain = { node, sourceGeneration: 12, seekSerial: 7, targetPositionSeconds: 10 };
    Object.defineProperties(node, {
      seeking: { configurable: true, value: true },
      readyState: { configurable: true, value: 1 },
    });
    expect(isFreshWebviewWaveformWindowAfterSeek(node, 12, 7, window, drain)).toBe(false);
    Object.defineProperty(node, "seeking", { configurable: true, value: false });
    expect(isFreshWebviewWaveformWindowAfterSeek(node, 12, 7, window, drain)).toBe(false);
    Object.defineProperty(node, "readyState", { configurable: true, value: 4 });
    expect(isFreshWebviewWaveformWindowAfterSeek(node, 12, 8, window, drain)).toBe(true);
  });
});
