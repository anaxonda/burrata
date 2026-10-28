export const BACKUP_FRONTEND_PREFERENCE_KEYS = new Set([
  "burrata.discovery.search-scope.v2", "burrata.discovery.sources.v1",
  "burrata.discovery.continue-search.v1", "burrata.discovery.search-history.v1",
  "burrata.soulseek.wishlist.v1", "burrata.queue-end-radio.v1",
  "burrata.waveform-enabled.v1", "burrata.stream-media-analysis-enabled.v1", "burrata.layout.sidebar.v1",
  "burrata.layout.info.v1", "burrata.lyrics-timing.v1",
  "burrata.playlist.include-ytdlp-search.v1",
]);

const booleanKeys = new Set([
  "burrata.discovery.continue-search.v1", "burrata.queue-end-radio.v1",
  "burrata.waveform-enabled.v1", "burrata.stream-media-analysis-enabled.v1",
  "burrata.playlist.include-ytdlp-search.v1",
]);

export const BACKUP_FRONTEND_PREFERENCE_RECEIPT_KEY = "burrata.backup-frontend-preference-receipt.v1";

export type PendingBackupFrontendPreferences = {
  nonce: string;
  currentSessionId: string;
  preferences: Record<string, unknown>;
};

export type BackupFrontendPreferenceAck = "acknowledged" | "deferred" | "stale";
export type BackupFrontendPreferenceHydration = "none" | BackupFrontendPreferenceAck;
export type BackupFrontendPreferenceStartupValues = ReadonlyMap<string, string>;

type StorageLike = Pick<Storage, "getItem" | "setItem">;

type Receipt = { nonce: string; sessionId: string };

function writeAndVerify(storage: StorageLike, key: string, value: string): void {
  storage.setItem(key, value);
  if (storage.getItem(key) !== value) throw new Error("Restored preferences could not be saved. Check available storage and try again.");
}

function isUuid(value: unknown): value is string {
  return typeof value === "string" && /^[0-9a-f]{8}-[0-9a-f]{4}-[1-8][0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/i.test(value);
}

function readReceipt(storage: StorageLike): Receipt | null {
  const raw = storage.getItem(BACKUP_FRONTEND_PREFERENCE_RECEIPT_KEY);
  if (!raw) return null;
  try {
    const value: unknown = JSON.parse(raw);
    if (typeof value !== "object" || value === null) return null;
    const record = value as Record<string, unknown>;
    return isUuid(record.nonce) && isUuid(record.sessionId)
      ? { nonce: record.nonce, sessionId: record.sessionId }
      : null;
  } catch {
    return null;
  }
}

function serializePreference(key: string, value: unknown): string {
  if (booleanKeys.has(key)) {
    if (typeof value !== "boolean") throw new Error(`Invalid restored value for ${key}`);
    return String(value);
  }
  if ((key === "burrata.layout.sidebar.v1" || key === "burrata.layout.info.v1") && typeof value === "number" && Number.isFinite(value)) {
    return String(value);
  }
  const encoded = JSON.stringify(value);
  if (encoded === undefined) throw new Error(`Could not encode restored value for ${key}`);
  return encoded;
}

/**
 * Apply allowlisted frontend settings and leave the native envelope pending until
 * a matching localStorage receipt is observed from a later backend process.
 */
export async function applyBackupFrontendPreferences(
  getPending: () => Promise<PendingBackupFrontendPreferences | null>,
  storage: StorageLike,
  acknowledge: (nonce: string, receiptSessionId: string) => Promise<BackupFrontendPreferenceAck>,
  onApplied?: (values: BackupFrontendPreferenceStartupValues) => void,
): Promise<BackupFrontendPreferenceHydration> {
  for (let attempt = 0; attempt < 3; attempt += 1) {
    const pending = await getPending();
    if (!pending) return "none";
    if (!isUuid(pending.nonce) || !isUuid(pending.currentSessionId) || typeof pending.preferences !== "object" || pending.preferences === null || Array.isArray(pending.preferences)) {
      throw new Error("Pending frontend preferences are invalid");
    }

    const receipt = readReceipt(storage);
    if (receipt?.nonce === pending.nonce) {
      const ack = await acknowledge(pending.nonce, receipt.sessionId);
      if (ack === "stale") continue;
      return ack;
    }

    const serializedValues = new Map<string, string>();
    for (const [key, value] of Object.entries(pending.preferences)) {
      if (!BACKUP_FRONTEND_PREFERENCE_KEYS.has(key)) continue;
      const serialized = serializePreference(key, value);
      writeAndVerify(storage, key, serialized);
      serializedValues.set(key, serialized);
    }
    writeAndVerify(storage, BACKUP_FRONTEND_PREFERENCE_RECEIPT_KEY, JSON.stringify({
      nonce: pending.nonce,
      sessionId: pending.currentSessionId,
    } satisfies Receipt));

    const ack = await acknowledge(pending.nonce, pending.currentSessionId);
    if (ack === "stale") continue;
    onApplied?.(serializedValues);
    return ack;
  }
  throw new Error("Imported frontend preferences changed repeatedly during hydration; retry to continue.");
}
