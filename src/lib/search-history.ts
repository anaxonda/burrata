export const SEARCH_HISTORY_KEY = "burrata.discovery.search-history.v1";
export const SEARCH_HISTORY_LIMIT = 20;

export interface SearchHistoryEntry {
  query: string;
  kind: string;
  mode: "online" | "library";
  sourceChoice: string;
  customSourceIds: string[];
  savedAt: number;
}

interface StoredSearchHistory { version: 1; entries: SearchHistoryEntry[] }

function normalized(query: string): string { return query.trim().replace(/\s+/g, " ").toLocaleLowerCase(); }

function validEntry(value: unknown): value is SearchHistoryEntry {
  if (!value || typeof value !== "object") return false;
  const entry = value as Partial<SearchHistoryEntry>;
  return typeof entry.query === "string" && Boolean(normalized(entry.query))
    && typeof entry.kind === "string" && ["all", "tracks", "artists", "albums", "videos", "playlists", "profiles", "channels"].includes(entry.kind)
    && (entry.mode === "online" || entry.mode === "library")
    && typeof entry.sourceChoice === "string"
    && Array.isArray(entry.customSourceIds) && entry.customSourceIds.every(id => typeof id === "string")
    && typeof entry.savedAt === "number" && Number.isFinite(entry.savedAt);
}

export function readSearchHistory(storage: Pick<Storage, "getItem"> | null = typeof localStorage === "undefined" ? null : localStorage): SearchHistoryEntry[] {
  if (!storage) return [];
  try {
    const parsed = JSON.parse(storage.getItem(SEARCH_HISTORY_KEY) ?? "null") as Partial<StoredSearchHistory> | null;
    if (!parsed || parsed.version !== 1 || !Array.isArray(parsed.entries)) return [];
    const unique = new Map<string, SearchHistoryEntry>();
    for (const item of parsed.entries) if (validEntry(item)) {
      const key = normalized(item.query);
      if (!unique.has(key)) unique.set(key, { ...item, query: item.query.trim().replace(/\s+/g, " "), customSourceIds: [...item.customSourceIds] });
    }
    return [...unique.values()].slice(0, SEARCH_HISTORY_LIMIT);
  } catch { return []; }
}

export function writeSearchHistory(entries: SearchHistoryEntry[], storage: Pick<Storage, "setItem"> | null = typeof localStorage === "undefined" ? null : localStorage): void {
  if (!storage) return;
  try { storage.setItem(SEARCH_HISTORY_KEY, JSON.stringify({ version: 1, entries: entries.slice(0, SEARCH_HISTORY_LIMIT) } satisfies StoredSearchHistory)); } catch { /* storage can be unavailable or full */ }
}

export function addSearchHistoryEntry(entries: SearchHistoryEntry[], entry: SearchHistoryEntry): SearchHistoryEntry[] {
  if (!validEntry(entry)) return entries.slice(0, SEARCH_HISTORY_LIMIT);
  const query = entry.query.trim().replace(/\s+/g, " ");
  const key = normalized(query);
  return [{ ...entry, query, customSourceIds: [...entry.customSourceIds] }, ...entries.filter(item => normalized(item.query) !== key)].slice(0, SEARCH_HISTORY_LIMIT);
}

export function removeSearchHistoryEntry(entries: SearchHistoryEntry[], query: string): SearchHistoryEntry[] {
  const key = normalized(query);
  return entries.filter(item => normalized(item.query) !== key).slice(0, SEARCH_HISTORY_LIMIT);
}
