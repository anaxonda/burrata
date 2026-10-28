/** WebView startup bookmark policy for native-engine playback.
 * Native queue/session data remains authoritative for native starts; this
 * bookmark carries the latest native cursor into a later WebView-engine start.
 */
export interface NativePositionBookmark {
  trackId: string;
  /** Optional for backward compatibility with track-only bookmarks. */
  entryId?: string;
  position: number;
  savedAt: number;
}

export interface NativePositionCheckpoint {
  trackId: string;
  entryId: string;
  generation: number;
  seekSerial: number;
  position: number;
  now: number;
  paused: boolean;
  radio: boolean;
}

export interface NativePositionOwner {
  trackId: string;
  entryId: string;
  generation: number;
  seekSerial: number;
  savedAt: number;
  paused: boolean;
}

export const NATIVE_POSITION_BOOKMARK_INTERVAL_MS = 10_000;

export function createNativePositionBookmark(checkpoint: NativePositionCheckpoint): NativePositionBookmark {
  return {
    trackId: checkpoint.trackId,
    entryId: checkpoint.entryId,
    position: checkpoint.position,
    savedAt: checkpoint.now,
  };
}

export function validNativePositionBookmark(value: unknown): value is NativePositionBookmark {
  if (!value || typeof value !== "object" || Array.isArray(value)) return false;
  const item = value as Partial<NativePositionBookmark>;
  return typeof item.trackId === "string" && item.trackId.length > 0 && item.trackId.length <= 512
    && (item.entryId === undefined || typeof item.entryId === "string" && item.entryId.length > 0 && item.entryId.length <= 128)
    && typeof item.position === "number" && Number.isFinite(item.position) && item.position >= 0 && item.position <= 86400
    && typeof item.savedAt === "number" && Number.isFinite(item.savedAt) && item.savedAt >= 0;
}

/** Occurrence-aware records must match the current duplicate occurrence.
 * Legacy records without entryId continue to match by track identity.
 */
export function nativeBookmarkMatchesOccurrence(
  bookmark: NativePositionBookmark,
  trackId: string | null | undefined,
  entryId: string | null | undefined,
): boolean {
  return bookmark.trackId === trackId && (bookmark.entryId === undefined || bookmark.entryId === entryId);
}

/** Checkpoint on owner changes, pause transitions, seeks, or after ten seconds.
 * Ephemeral live-radio cursors are excluded.
 */
export function shouldPersistNativePositionBookmark(
  checkpoint: NativePositionCheckpoint,
  previous: NativePositionOwner | null,
  force = false,
): boolean {
  if (checkpoint.radio || !checkpoint.trackId || !checkpoint.entryId
      || !Number.isSafeInteger(checkpoint.generation) || checkpoint.generation < 0
      || !Number.isSafeInteger(checkpoint.seekSerial) || checkpoint.seekSerial < 0
      || !Number.isFinite(checkpoint.position) || checkpoint.position < 0 || checkpoint.position > 86400
      || !Number.isFinite(checkpoint.now) || checkpoint.now < 0) return false;
  if (force || !previous || checkpoint.paused && !previous.paused) return true;
  if (checkpoint.trackId !== previous.trackId || checkpoint.entryId !== previous.entryId
      || checkpoint.generation !== previous.generation || checkpoint.seekSerial !== previous.seekSerial) return true;
  return checkpoint.now >= previous.savedAt && checkpoint.now - previous.savedAt >= NATIVE_POSITION_BOOKMARK_INTERVAL_MS;
}
