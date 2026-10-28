import type { PlaybackQueueSnapshot } from "./playback-queue";
import type { Track } from "./types";

export interface PendingBackupPlaybackRestore {
  version: 1;
  nonce: string;
  currentSessionId: string;
  queue: PlaybackQueueSnapshot;
  position: number;
  volume: number;
  repeat: "off" | "all" | "one";
  shuffle: boolean;
  paused: true;
}

export type BackupPlaybackRestoreAck = "acknowledged" | "deferred" | "stale";
export type BackupPlaybackStartupValues = ReadonlyMap<string, string>;

export const BACKUP_PLAYBACK_RESTORE_RECEIPT_KEY = "burrata.backup-playback-restore-receipt.v1";

const QUEUE_KEY = "burrata.playback-queue.v1";
const POSITION_KEY = "burrata.playback-position.v1";
const DURABLE_CONTROLS_KEY = "burrata.playback-controls.v1";
const ENGINE_CONTROLS_KEY = "burrata.engine-switch-controls.v1";
const HYDRATION_ATTEMPTS = 3;
const PLAYBACK_STORAGE_READBACK_ERROR = "Restored playback could not be saved. Check available storage and try again.";

type StorageLike = Pick<Storage, "getItem" | "setItem">;
type Receipt = { nonce: string; sessionId: string };

function writeAndVerify(storage: StorageLike, key: string, value: string): void {
  storage.setItem(key, value);
  if (storage.getItem(key) !== value) throw new Error(PLAYBACK_STORAGE_READBACK_ERROR);
}

function isUuid(value: unknown): value is string {
  return typeof value === "string" && /^[0-9a-f]{8}-[0-9a-f]{4}-[1-8][0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/i.test(value);
}

function validQueue(value: unknown): value is PlaybackQueueSnapshot {
  if (!value || typeof value !== "object" || Array.isArray(value)) return false;
  const queue = value as Partial<PlaybackQueueSnapshot>;
  if (!Array.isArray(queue.tracks) || !Array.isArray(queue.entryIds)
      || queue.tracks.length !== queue.entryIds.length
      || !Number.isInteger(queue.currentIndex)
      || !["off", "all", "one"].includes(queue.repeat ?? "")) return false;
  const ids = queue.entryIds as unknown[];
  if (!ids.every(id => typeof id === "string" && id.length > 0 && id.length <= 128 && !/[\u0000-\u001f\u007f]/.test(id))
      || new Set(ids).size !== ids.length) return false;
  if (queue.tracks.some(track => !track || typeof track !== "object" || typeof (track as Track).id !== "string" || !(track as Track).id)) return false;
  return queue.tracks.length === 0
    ? queue.currentIndex === 0 || queue.currentIndex === -1
    : queue.currentIndex! >= 0 && queue.currentIndex! < queue.tracks.length;
}

function validHandoff(value: unknown): value is PendingBackupPlaybackRestore {
  if (!value || typeof value !== "object" || Array.isArray(value)) return false;
  const handoff = value as Partial<PendingBackupPlaybackRestore>;
  return handoff.version === 1
    && isUuid(handoff.nonce)
    && isUuid(handoff.currentSessionId)
    && validQueue(handoff.queue)
    && typeof handoff.position === "number" && Number.isFinite(handoff.position) && handoff.position >= 0
    && typeof handoff.volume === "number" && Number.isFinite(handoff.volume) && handoff.volume >= 0 && handoff.volume <= 1
    && ["off", "all", "one"].includes(handoff.repeat ?? "")
    && typeof handoff.shuffle === "boolean"
    && handoff.paused === true;
}

function readReceipt(storage: StorageLike): Receipt | null {
  try {
    const raw = storage.getItem(BACKUP_PLAYBACK_RESTORE_RECEIPT_KEY);
    if (!raw) return null;
    const value: unknown = JSON.parse(raw);
    if (!value || typeof value !== "object" || Array.isArray(value)) return null;
    const record = value as Record<string, unknown>;
    return isUuid(record.nonce) && isUuid(record.sessionId)
      ? { nonce: record.nonce, sessionId: record.sessionId }
      : null;
  } catch {
    return null;
  }
}

/**
 * Persist a selected playback restore to WebView storage and keep the native
 * marker pending until a matching receipt is observed in a later backend
 * process. The receipt is written last and matching receipts are never replayed,
 * preserving edits made after the restore.
 */
export async function applyPendingBackupPlaybackRestore(
  getPending: () => Promise<unknown>,
  storage: StorageLike,
  acknowledge: (nonce: string, receiptSessionId: string) => Promise<BackupPlaybackRestoreAck>,
  onApplied?: (values: BackupPlaybackStartupValues) => void,
): Promise<boolean> {
  for (let attempt = 0; attempt < HYDRATION_ATTEMPTS; attempt += 1) {
    const pending = await getPending();
    if (pending == null) return false;
    if (!validHandoff(pending)) throw new Error("Invalid pending playback restore");

    const receipt = readReceipt(storage);
    if (receipt?.nonce === pending.nonce) {
      const ack = await acknowledge(pending.nonce, receipt.sessionId);
      if (ack === "stale") continue;
      return true;
    }

    const { queue, position, volume, repeat, shuffle, nonce } = pending;
    const current = queue.tracks[queue.currentIndex];
    const nextQueue: PlaybackQueueSnapshot = { ...queue, repeat };
    const values = new Map<string, string>();
    // These are WebView persistence keys only. Native playback continues to load
    // its validated database snapshot and will publish its own state on startup.
    const serializedQueue = JSON.stringify(nextQueue);
    const serializedPosition = JSON.stringify({ trackId: current?.id ?? null, position, savedAt: Date.now() });
    writeAndVerify(storage, QUEUE_KEY, serializedQueue);
    writeAndVerify(storage, POSITION_KEY, serializedPosition);
    // Durable controls survive ordinary cold WebView starts. Keep the legacy
    // engine-switch handoff synchronized so it cannot override this restore.
    const controls = JSON.stringify({ volume, shuffle });
    writeAndVerify(storage, DURABLE_CONTROLS_KEY, controls);
    writeAndVerify(storage, ENGINE_CONTROLS_KEY, controls);
    values.set(QUEUE_KEY, serializedQueue);
    values.set(POSITION_KEY, serializedPosition);
    values.set(DURABLE_CONTROLS_KEY, controls);
    values.set(ENGINE_CONTROLS_KEY, controls);
    // If a browser reports success for a memory-only write, a durable receipt
    // will be absent on restart and this handoff will be applied again.
    writeAndVerify(storage, BACKUP_PLAYBACK_RESTORE_RECEIPT_KEY, JSON.stringify({
      nonce: pending.nonce,
      sessionId: pending.currentSessionId,
    } satisfies Receipt));

    const ack = await acknowledge(nonce, pending.currentSessionId);
    if (ack === "stale") continue;
    onApplied?.(values);
    return true;
  }
  throw new Error("Imported playback changed repeatedly during hydration; retry to continue.");
}
