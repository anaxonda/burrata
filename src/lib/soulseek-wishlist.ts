export const WISHLIST_KEY = "burrata.soulseek.wishlist.v1";
export const WISHLIST_INTERVAL = 60 * 60 * 1000;
export const WISHLIST_GAP = 60 * 1000;
export interface WishlistEntry {
  id: string; query: string; enabled: boolean; nextRun: number;
  failures: number; lastChecked: number | null; resultCount: number | null;
  outcome: "unchecked" | "results" | "empty" | "unavailable";
}
type Persistence = Pick<Storage, "getItem" | "setItem">;
const key = (query: string) => query.trim().replace(/\s+/g, " ").toLowerCase();
function valid(value: unknown): value is WishlistEntry {
  if (!value || typeof value !== "object") return false;
  const e = value as WishlistEntry;
  return typeof e.id === "string" && e.id.length > 0 && e.id.length < 100
    && typeof e.query === "string" && e.query.trim().length > 0 && e.query.length <= 500
    && typeof e.enabled === "boolean" && Number.isSafeInteger(e.nextRun) && e.nextRun >= 0
    && Number.isInteger(e.failures) && e.failures >= 0 && e.failures <= 10
    && (e.lastChecked === null || Number.isSafeInteger(e.lastChecked) && e.lastChecked >= 0)
    && (e.resultCount === null || Number.isInteger(e.resultCount) && e.resultCount >= 0 && e.resultCount <= 10000)
    && ["unchecked", "results", "empty", "unavailable"].includes(e.outcome);
}
export class SoulseekWishlist {
  entries: WishlistEntry[] = [];
  private busy: { id: string; controller: AbortController } | null = null;
  private nextSearch = 0;
  constructor(private storage: Persistence, private now = Date.now, private changed = () => {}) {
    try {
      const saved = JSON.parse(storage.getItem(WISHLIST_KEY) ?? "null");
      if (saved?.version === 1 && Array.isArray(saved.entries)) {
        if (Number.isSafeInteger(saved.nextSearch) && saved.nextSearch >= 0) this.nextSearch = Math.min(saved.nextSearch, this.now() + WISHLIST_GAP);
        const seen = new Set<string>();
        const ids = new Set<string>();
        this.entries = saved.entries.filter(valid).filter((entry: WishlistEntry) => {
          const normalized = key(entry.query);
          if (seen.has(normalized) || ids.has(entry.id)) return false;
          seen.add(normalized); ids.add(entry.id); return true;
        }).slice(0, 50);
      }
    } catch { /* Corrupt storage never starts a search. */ }
  }
  private save(entries: WishlistEntry[]) {
    // Persist before scheduling network work; failed writes must be visible.
    this.storage.setItem(WISHLIST_KEY, JSON.stringify({version: 1, entries, nextSearch: this.nextSearch}));
    this.entries = entries; this.changed();
  }
  add(query: string) {
    query = query.trim().replace(/\s+/g, " ");
    if (!query || query.length > 500) throw new Error("Enter a search of 1 to 500 characters.");
    if (this.entries.some(entry => key(entry.query) === key(query))) return;
    if (this.entries.length >= 50) throw new Error("The wishlist can contain up to 50 searches.");
    this.save([...this.entries, {id: crypto.randomUUID(), query, enabled: true, nextRun: this.now(), failures: 0, lastChecked: null, resultCount: null, outcome: "unchecked"}]);
  }
  enable(id: string, enabled: boolean) {
    this.save(this.entries.map(e => e.id === id ? {...e, enabled} : e));
    if (!enabled && this.busy?.id === id) this.busy.controller.abort();
  }
  remove(id: string) {
    this.save(this.entries.filter(e => e.id !== id));
    if (this.busy?.id === id) this.busy.controller.abort();
  }
  stop() { this.busy?.controller.abort(); }
  async tick(search: (query: string, signal: AbortSignal) => Promise<number>) {
    const now = this.now();
    if (this.busy || now < this.nextSearch) return;
    const entry = this.entries.filter(e => e.enabled && e.nextRun <= now).sort((a,b) => a.nextRun-b.nextRun)[0];
    if (!entry) return;
    // Persist reservation first so restart/reload does not repeat immediate work.
    this.nextSearch = now + WISHLIST_GAP;
    this.save(this.entries.map(e => e.id === entry.id ? {...e, nextRun: now + WISHLIST_INTERVAL} : e));
    const controller = new AbortController();
    this.busy = {id: entry.id, controller};
    try {
      const count = await search(entry.query, controller.signal);
      if (controller.signal.aborted) return;
      if (!Number.isInteger(count) || count < 0 || count > 10000) throw new Error("Invalid search result count");
      this.save(this.entries.map(e => e.id === entry.id ? {...e, failures: 0, lastChecked: this.now(), resultCount: count, outcome: count ? "results" : "empty"} : e));
    } catch {
      if (controller.signal.aborted) return;
      this.save(this.entries.map(e => e.id === entry.id ? {...e, failures: Math.min(10, e.failures + 1), lastChecked: this.now(), resultCount: null, outcome: "unavailable", nextRun: this.now() + Math.min(24 * WISHLIST_INTERVAL, WISHLIST_INTERVAL * 2 ** Math.min(5, e.failures))} : e));
    } finally { this.busy = null; }
  }
}
