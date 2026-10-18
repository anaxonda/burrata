/** Memory-only, account-scoped snapshots for provider-native browsing routes. */
export type ProviderSnapshotKind = "playlists" | "playlist-detail" | "spotify-artist" | "artist-page" | "search-panel" | "search-release";
export type ProviderSnapshotEntry<T> = {
  kind: ProviderSnapshotKind;
  key: string;
  accountScope: string;
  snapshot: T;
  rows: number;
  stripRows: (snapshot: T) => T;
};

export class ProviderNavigationSnapshots {
  private entries = new Map<string, ProviderSnapshotEntry<unknown>>();
  // Small sanitized controls/identity records follow the bounded app route history
  // after a heavy page set is evicted. They never retain provider rows.
  private controls = new Map<string, { kind: ProviderSnapshotKind; key: string; accountScope: string; snapshot: unknown }>();
  private rowCount = 0;
  constructor(readonly maxEntries = 20, readonly maxRows = 20_000) {}

  put<T>(entry: ProviderSnapshotEntry<T>) {
    const cacheKey = `${entry.kind}:${entry.key}`;
    const previous = this.entries.get(cacheKey);
    if (previous) this.rowCount -= previous.rows;
    // Keep the caller's arrays by reference. Snapshot records are shallow values; rows are
    // immutable provider results and are never cloned during scroll or playback updates.
    this.entries.delete(cacheKey);
    this.entries.set(cacheKey, entry as unknown as ProviderSnapshotEntry<unknown>);
    this.rowCount += entry.rows;
    this.controls.delete(cacheKey);
    this.controls.set(cacheKey, { kind: entry.kind, key: entry.key, accountScope: entry.accountScope, snapshot: entry.stripRows(entry.snapshot) });
    while (this.controls.size > 100) this.controls.delete(this.controls.keys().next().value as string);
    this.trim();
  }

  get<T>(kind: ProviderSnapshotKind, key: string, accountScope: string | null): T | null {
    const cacheKey = `${kind}:${key}`;
    const entry = this.entries.get(cacheKey);
    if (!entry) {
      const controls = this.controls.get(cacheKey);
      if (!controls) return null;
      if (accountScope) controls.accountScope = accountScope;
      this.controls.delete(cacheKey); this.controls.set(cacheKey, controls);
      return controls.snapshot as T;
    }
    if (!accountScope || entry.accountScope !== accountScope) {
      this.rowCount -= entry.rows;
      entry.snapshot = entry.stripRows(entry.snapshot);
      entry.rows = 0;
      if (accountScope) entry.accountScope = accountScope;
      const controls = this.controls.get(cacheKey);
      if (controls) { controls.snapshot = entry.snapshot; if (accountScope) controls.accountScope = accountScope; }
    }
    this.entries.delete(cacheKey);
    this.entries.set(cacheKey, entry);
    return entry.snapshot as T;
  }

  invalidateAccount(kind: ProviderSnapshotKind, accountScope?: string) {
    for (const [key, entry] of this.entries) {
      if (entry.kind === kind && (accountScope === undefined || entry.accountScope === accountScope)) {
        this.entries.delete(key);
        this.rowCount -= entry.rows;
      }
    }
  }

  clear() {
    this.entries.clear();
    this.controls.clear();
    this.rowCount = 0;
  }

  get size() { return this.entries.size; }
  get rows() { return this.rowCount; }

  private trim() {
    while (this.entries.size > this.maxEntries) this.removeOldest();
    // Strip accumulated provider rows first while retaining route controls and loaded count.
    for (const entry of this.entries.values()) {
      if (this.rowCount <= this.maxRows) break;
      if (entry.rows > 0) {
        entry.snapshot = entry.stripRows(entry.snapshot);
        this.rowCount -= entry.rows;
        entry.rows = 0;
      }
    }
    while (this.rowCount > this.maxRows && this.entries.size) this.removeOldest();
  }

  private removeOldest() {
    const oldest = this.entries.keys().next().value as string | undefined;
    if (oldest === undefined) return;
    const entry = this.entries.get(oldest);
    if (entry) this.rowCount -= entry.rows;
    this.entries.delete(oldest);
  }
}

export const providerNavigationSnapshots = new ProviderNavigationSnapshots();

/** Existing provider-session signals only; unknown identity deliberately disables reuse. */
export async function providerAccountScope(
  call: (command: string, args: Record<string, unknown>) => Promise<unknown>,
  provider: "spotify" | "youtube",
): Promise<string | null> {
  try {
    const session = await call(
      "get_managed_provider_session_status", {}) as {
      accountRevisions?: Record<string, number>;
      spotifyAccountId?: string | null;
      youtubeAccountId?: string | null;
      youtubeReconnectRequired?: boolean;
      enabled?: boolean;
      running?: boolean;
      mode?: string;
    };
    // Preserve compatibility with older sparse fixtures/backends, but explicit
    // managed-session shutdown states override lingering account IDs. Do not let
    // the separate connection-status fallback reauthorize a stopped transport.
    const explicitlyUnavailable = session.enabled === false || session.running === false || session.mode === "stopped" || session.mode === "native-confirmed";
    if (explicitlyUnavailable || (provider === "youtube" && session.youtubeReconnectRequired)) return null;
    const accountId = provider === "spotify" ? session.spotifyAccountId : session.youtubeAccountId;
    const revision = session.accountRevisions?.[provider];
    if (accountId && Number.isSafeInteger(revision) && revision! >= 0) return `${provider}:revision:${revision}:${accountId}`;
    if (accountId) return `${provider}:account:${accountId}`;
  } catch { /* Older backends expose connection status only. */ }
  try {
    const status = await call("remote_playlist_connection_status", { provider }) as { accountId?: string | null; connected?: boolean };
    return status.connected !== false && status.accountId ? `${provider}:account:${status.accountId}` : null;
  } catch { return null; }
}
