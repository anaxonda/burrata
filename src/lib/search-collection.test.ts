import { describe, expect, it } from "vitest";
import { splitCollectionRows, takeCollectionRows } from "./search-collection";

describe("takeCollectionRows", () => {
  it("counts every result kind against an action budget and preserves overflow order", () => {
    const rows = ["track", "artist", "album", "playlist"].map((kind, index) => ({ id: String(index), kind }));
    expect(takeCollectionRows(rows, 2)).toEqual({ accepted: rows.slice(0, 2), overflow: rows.slice(2) });
    expect(takeCollectionRows(rows, 0)).toEqual({ accepted: [], overflow: rows });
  });
});

describe("splitCollectionRows", () => {
  it("keeps page overflow buffered so a later manual load can resume without losing rows", () => {
    const page = ["a", "b", "c", "d"].map(id => ({ id, sourceId: "youtube", kind: "track", track: {} }));
    const first = splitCollectionRows([], page, 2);
    expect(first.accepted).toEqual(page.slice(0, 2));
    expect(first.overflow).toEqual(page.slice(2));
    const resumedManually = first.overflow.slice(0, 1);
    expect(resumedManually).toEqual([page[2]]);
    expect(first.overflow.slice(resumedManually.length)).toEqual([page[3]]);
  });

  it("honors the track cap while retaining non-track rows and stable duplicates", () => {
    const existing = [
      { id: "a", sourceId: "youtube", kind: "track", track: {} },
      { id: "b", sourceId: "spotify", kind: "track", track: {} },
    ];
    const incoming = [
      { id: "a", sourceId: "youtube", kind: "track", track: {} },
      { id: "c", sourceId: "youtube", kind: "track", track: {} },
      { id: "artist", sourceId: "spotify", kind: "artist" },
      { id: "d", sourceId: "spotify", kind: "track", track: {} },
    ];
    const three = splitCollectionRows(existing, incoming, 3);
    expect(three.accepted).toEqual(incoming.slice(0, 3));
    expect(three.overflow).toEqual([incoming[3]]);
    const two = splitCollectionRows(existing, incoming, 2);
    expect(two.accepted).toEqual([incoming[0], incoming[2]]);
    expect(two.overflow).toEqual([incoming[1], incoming[3]]);
    const one = splitCollectionRows(existing, incoming, 1);
    expect(one.accepted).toEqual([incoming[0], incoming[2]]);
    expect(one.overflow).toEqual([incoming[1], incoming[3]]);
  });
  it("does not admit a duplicate overflow row past the cap", () => {
    const row = (id: string) => ({ id, sourceId: "youtube", kind: "track", track: {} });
    const page = [row("a"), row("b"), row("b")];
    const split = splitCollectionRows([], page, 1);
    expect(split.accepted).toEqual([page[0]]);
    expect(split.overflow).toEqual(page.slice(1));
    const resumed = splitCollectionRows(split.accepted, split.overflow, 2);
    expect(new Set([...split.accepted, ...resumed.accepted].map(row => row.id)).size).toBe(2);
    expect(resumed.overflow).toEqual([]);
  });

});
