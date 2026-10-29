import { describe, expect, it } from "vitest";
import { historyOccurrenceDateRange } from "./history-occurrence-range";

function expectIncludesInstant(playedAt: string, from: string, to: string) {
  const instant = Date.parse(playedAt);
  const fromStart = Date.parse(`${from}T00:00:00.000Z`);
  const toEndExclusive = Date.parse(`${to}T00:00:00.000Z`) + 24 * 60 * 60 * 1000;
  expect(instant).toBeGreaterThanOrEqual(fromStart);
  expect(instant).toBeLessThan(toEndExclusive);
}

describe("history occurrence UTC date range", () => {
  it("includes a late local-night playback whose UTC date is already the next day", () => {
    const playedAt = "2026-01-01T06:30:00.000Z"; // December 31 at 11:30 PM in Denver.
    const range = historyOccurrenceDateRange(playedAt);
    expect(range).toEqual({ from: "2025-12-31", to: "2026-01-02" });
    expectIncludesInstant(playedAt, range!.from, range!.to);
  });

  it("crosses a month boundary and contains the exact UTC occurrence instant", () => {
    const playedAt = "2026-03-01T00:20:00.000Z";
    const range = historyOccurrenceDateRange(playedAt);
    expect(range).toEqual({ from: "2026-02-28", to: "2026-03-02" });
    expectIncludesInstant(playedAt, range!.from, range!.to);
  });

  it("crosses a year boundary and contains the exact UTC occurrence instant", () => {
    const playedAt = "2026-01-01T00:15:00.000Z";
    const range = historyOccurrenceDateRange(playedAt);
    expect(range).toEqual({ from: "2025-12-31", to: "2026-01-02" });
    expectIncludesInstant(playedAt, range!.from, range!.to);
  });

  it("rejects missing or invalid occurrence timestamps", () => {
    expect(historyOccurrenceDateRange("")).toBeNull();
    expect(historyOccurrenceDateRange("not-a-timestamp")).toBeNull();
  });
});
