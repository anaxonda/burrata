import { describe, expect, it } from "vitest";
import { addSearchHistoryEntry, readSearchHistory, removeSearchHistoryEntry, SEARCH_HISTORY_KEY, SEARCH_HISTORY_LIMIT, writeSearchHistory, type SearchHistoryEntry } from "./search-history";

function entry(query: string, savedAt = 1): SearchHistoryEntry {
  return { query, kind: "all", mode: "online", sourceChoice: "all", customSourceIds: [], savedAt };
}

describe("search history", () => {
  it("deduplicates normalized queries, moves repeats to the front, and bounds entries", () => {
    let items = addSearchHistoryEntry([], entry("  Nipsey   Hussle ", 1));
    items = addSearchHistoryEntry(items, entry("other", 2));
    items = addSearchHistoryEntry(items, entry("nipsey hussle", 3));
    expect(items.map(item => item.query)).toEqual(["nipsey hussle", "other"]);
    for (let i = 0; i < SEARCH_HISTORY_LIMIT + 5; i++) items = addSearchHistoryEntry(items, entry(`query ${i}`, i + 4));
    expect(items).toHaveLength(SEARCH_HISTORY_LIMIT);
    expect(items[0]?.query).toBe(`query ${SEARCH_HISTORY_LIMIT + 4}`);
  });

  it("handles corrupt storage and supports individual removal and clear persistence", () => {
    const values = new Map<string, string>([[SEARCH_HISTORY_KEY, "not json"]]);
    const storage = { getItem: (key: string) => values.get(key) ?? null, setItem: (key: string, value: string) => values.set(key, value) };
    expect(readSearchHistory(storage)).toEqual([]);
    const items = [entry("one"), entry("two")];
    writeSearchHistory(items, storage);
    expect(readSearchHistory(storage)).toEqual(items);
    const remaining = removeSearchHistoryEntry(items, " ONE ");
    writeSearchHistory(remaining, storage);
    expect(readSearchHistory(storage).map(item => item.query)).toEqual(["two"]);
    writeSearchHistory([], storage);
    expect(readSearchHistory(storage)).toEqual([]);
  });
});
