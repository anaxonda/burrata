import { describe, expect, it } from "vitest";
import { previousRadioDetections } from "./radio-queue";

const row = (id: string, sessionId: string, stationId: string, playedAt: string) => ({
  id,
  occurrenceId: id,
  sessionId,
  stationId,
  track: { id: "same-track" },
  playedAt,
  stationName: "Station",
});

describe("previousRadioDetections", () => {
  it("keeps only this session and station, in chronological order, excluding exact current occurrence", () => {
    const rows = [
      row("current", "session-1", "station-1", "2026-11-11T12:02:00Z"),
      row("latest-prior", "session-1", "station-1", "2026-11-11T12:01:00Z"),
      row("old-session", "session-0", "station-1", "2026-11-11T11:59:00Z"),
      row("other-station", "session-1", "station-2", "2026-11-11T12:00:00Z"),
      row("oldest-prior", "session-1", "station-1", "2026-11-11T12:00:00Z"),
    ];

    expect(previousRadioDetections(rows, "session-1", "station-1", "current").map((item) => item.id))
      .toEqual(["oldest-prior", "latest-prior"]);
  });

  it("preserves repeated track occurrences and remains stable when current is unknown", () => {
    const rows = [
      row("repeat-2", "session-1", "station-1", "2026-11-11T12:01:00Z"),
      row("repeat-1", "session-1", "station-1", "2026-11-11T12:00:00Z"),
    ];
    expect(previousRadioDetections(rows, "session-1", "station-1", null).map((item) => item.id))
      .toEqual(["repeat-1", "repeat-2"]);
    expect(previousRadioDetections(rows, "session-2", "station-1", null)).toEqual([]);
  });
});
