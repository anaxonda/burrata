import { describe, expect, it } from "vitest";
import { isStreamMediaAnalysisPaused } from "./stream-analysis-policy";

describe("stream media analysis restore policy", () => {
  it("ignores review requirements for unrelated automatic-work categories", () => {
    expect(isStreamMediaAnalysisPaused({
      reviewRequired: true,
      categories: [
        { kind: "background_stream_media_analysis", paused: false },
        { kind: "background_source_preparation", paused: true },
      ],
    })).toBe(false);
  });

  it("pauses the stream category while leaving manual analysis available to its caller", () => {
    expect(isStreamMediaAnalysisPaused({
      reviewRequired: false,
      categories: [{ kind: "background_stream_media_analysis", paused: true }],
    })).toBe(true);
  });

  it("fails closed for older policies that omit the stream category", () => {
    expect(isStreamMediaAnalysisPaused({
      reviewRequired: true,
      categories: [{ kind: "background_source_preparation", paused: true }],
    })).toBe(true);
  });
});
