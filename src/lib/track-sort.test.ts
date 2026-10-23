import { describe, expect, it } from "vitest";
import fixtures from "../../tests/fixtures/track-sort-conformance.json";
import {
  compareTrackText, filterTracks, normalizeSortCount, normalizeSortDate, parseDurationBound,
  playableDuration, sortTracks, TRACK_SORT_LOCALE,
} from "./track-sort";
import type { Track } from "./types";

function track(id: string, fields: Partial<Track> = {}): Track {
  return { id, provider: "test", providerId: id, sourceUrl: "", artist: "", title: id, favorite: false, provenance: {}, ...fields };
}

describe("sortTracks", () => {
  it("matches shared Unicode query, ICU base collation, numeric, and occurrence fixtures", () => {
    expect(fixtures.locale).toBe(TRACK_SORT_LOCALE);
    for (const testCase of fixtures.queryCases) {
      expect(filterTracks([track("fixture", { title: testCase.text })], { query: testCase.query }).length > 0)
        .toBe(testCase.matches);
    }
    for (const testCase of fixtures.sortCases) {
      const rows = testCase.input.map((title, index) => track(String(index), { title }));
      expect(sortTracks(rows, "title").map(row => row.title)).toEqual(testCase.expected);
    }
    for (const [left, right] of fixtures.equalPairs) {
      expect(sortTracks([track("left", { title: left }), track("right", { title: right })], "title").map(row => row.title))
        .toEqual([left, right]);
      expect(compareTrackText(left, right)).toBe(0);
    }
    const tied = fixtures.stableTies.map((title, index) => track(String(index), { title }));
    expect(sortTracks(tied, "title").map(row => row.title)).toEqual(fixtures.stableTies);
  });

  it("sorts text without mutating and keeps duplicate occurrences stable", () => {
    const same = track("same", { title: "Beta" });
    const input = [track("z", { title: "zeta" }), same, track("a", { title: "Alpha" }), same];
    expect(sortTracks(input, "title").map((t) => t.id)).toEqual(["a", "same", "same", "z"]);
    expect(input[0].id).toBe("z");
  });

  it("leaves unknowns last in either direction", () => {
    const input = [track("missing"), track("low", { bpm: 90 }), track("high", { bpm: 120 })];
    expect(sortTracks(input, "bpm", "desc").map((t) => t.id)).toEqual(["high", "low", "missing"]);
  });

  it("uses chapter length and compares duration at whole-second precision", () => {
    const input = [
      track("long", { durationSeconds: 90 }),
      track("chapter", { playbackRange: { parentTrackId: "p", chapterId: "c", sourceUrl: "", startTime: 12.1, endTime: 42.9, parentTitle: "p" } }),
      track("short", { durationSeconds: 30.1 }),
    ];
    expect(sortTracks(input, "duration").map((t) => t.id)).toEqual(["chapter", "short", "long"]);
    expect(playableDuration(input[1])).toBeCloseTo(30.8);
    expect(playableDuration(track("zero", { durationSeconds: 0 }))).toBeNull();
  });

  it("sorts musical keys chromatically and source order by occurrence", () => {
    expect(sortTracks([track("g", { key: "G minor" }), track("c", { key: "C major" }), track("d", { key: "D♭" })], "key").map((t) => t.id)).toEqual(["c", "d", "g"]);
    expect(sortTracks([track("flat", { key: "D♭" }), track("sharp", { key: "C# major" })], "key").map((t) => t.id)).toEqual(["flat", "sharp"]);
    expect(sortTracks([track("camelot", { key: "8A" }), track("relative", { key: "A minor" })], "key").map((t) => t.id)).toEqual(["camelot", "relative"]);
    const input = [track("a"), track("b"), track("c")];
    expect(sortTracks(input, "sourceOrder", "desc").map((t) => t.id)).toEqual(["c", "b", "a"]);
  });

  it("reads date added and play/view counts from provenance", () => {
    const input = [
      track("later", { provenance: { addedAt: "2024-02-01", playCount: 10, viewCount: 100 } }),
      track("earlier", { provenance: { dateAdded: "2024-01-01", plays: 20, views: 200 } }),
    ];
    expect(sortTracks(input, "dateAdded").map((t) => t.id)).toEqual(["earlier", "later"]);
    expect(sortTracks(input, "plays", "desc").map((t) => t.id)).toEqual(["earlier", "later"]);
    expect(sortTracks(input, "views").map((t) => t.id)).toEqual(["later", "earlier"]);
    const compact = [track("compact", { provenance: { views: "3.8M views" } }), track("exact", { provenance: { views: 4200000 } })];
    expect(sortTracks(compact, "views").map((t) => t.id)).toEqual(["compact", "exact"]);
  });

  it("matches shared numeric, key, and strict date normalization fixtures", () => {
    const cases = fixtures.sortNormalizationCases;
    for (const item of cases.numeric) {
      const actual = normalizeSortCount(item.value);
      expect(actual !== null).toBe(item.valid);
      expect(actual).toBe(item.valid ? item.expected ?? actual : null);
    }
    for (const item of cases.camelot) {
      const t = track("case", { key: item.value });
      const known = sortTracks([track("unknown"), t], "key")[0].id === "case";
      expect(known).toBe(item.valid);
    }
    for (const item of cases.dates) {
      const actual = normalizeSortDate(item.value);
      expect(actual !== null).toBe(item.valid);
      expect(actual).toBe(item.valid ? item.expected ?? actual : null);
    }
  });

  it("filters text, BPM, and whole-second duration without collapsing repeated tracks", () => {
    const repeated = track("x", { title: "Night Mix", artist: "Ada", bpm: 120, durationSeconds: 60.9 });
    const input = [repeated, track("y", { title: "Day", bpm: 90, durationSeconds: 61 }), repeated];
    expect(filterTracks(input, { query: "night", minBpm: 110, maxDurationSeconds: 60 }).length).toBe(2);
  });

  it("parses duration bounds in seconds or mm:ss", () => {
    expect(parseDurationBound("3:07")).toBe(187);
    expect(parseDurationBound("187.8")).toBe(187);
    expect(parseDurationBound("2:61")).toBeNull();
  });
});

describe("downloaded status filtering", () => {
  it("uses positive local availability, preserves repeated occurrences and rejects explicitly missing files", () => {
    const available=track("available",{localAvailability:"available"});
    const legacy=track("legacy",{localPath:"/music/legacy.flac"});
    const missing=track("missing",{localAvailability:"missing",localPath:"/music/missing.flac"});
    const remote=track("remote",{assetId:"unavailable-asset",localAvailability:"none"});
    const rows=[available,missing,legacy,remote,available];
    expect(filterTracks(rows,{availability:"downloaded"})).toEqual([available,legacy,available]);
    expect(filterTracks(rows,{availability:"not-downloaded"})).toEqual([missing,remote]);
    expect(rows).toHaveLength(5);
  });
});
