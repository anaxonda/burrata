import { describe, expect, it } from "vitest";
import { invertSelection } from "./selection";

describe("invertSelection", () => {
  it("inverts membership in the supplied scope and keeps selections outside it", () => {
    expect(invertSelection(["a", "b", "c"], ["b", "outside"])).toEqual(["a", "c", "outside"]);
  });

  it("handles repeated occurrences as distinct keys and de-duplicates repeated keys", () => {
    expect(invertSelection(["track:0", "track:1", "other", "track:1"], ["track:1", "track:1"])).toEqual(["track:0", "other"]);
  });

  it("selects the whole scope from an empty selection and clears a fully selected scope", () => {
    expect(invertSelection(["one", "two"], [])).toEqual(["one", "two"]);
    expect(invertSelection(["one", "two"], ["one", "two"])).toEqual([]);
  });
});
