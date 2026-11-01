import type { NativePlaybackSnapshot } from "./types";
import type { PlaybackQueueSnapshot } from "./playback-queue";

/** Reject partial/paged snapshots: switching engines must preserve every occurrence. */
export function queueForEngineMigration(snapshot: NativePlaybackSnapshot & {
  queueTotal?: number; queueEntryIds?: string[];
}): PlaybackQueueSnapshot {
  const {queue, queueEntryIds: ids, currentIndex, repeat} = snapshot;
  const queueLoudnessContexts = snapshot.queueLoudnessContexts;
  if (!Array.isArray(queue) || snapshot.queueTotal !== queue.length
    || !Array.isArray(ids) || ids.length !== queue.length
    || ids.some(id => typeof id !== "string" || !id.trim().length || id.length > 128 || /[\u0000-\u001f\u007f]/.test(id))
    || new Set(ids).size !== ids.length
    || !Number.isInteger(currentIndex) || currentIndex < -1 || currentIndex >= queue.length
    || (queue.length > 0 && currentIndex < 0)
    || !["off", "all", "one"].includes(repeat)
    || !Number.isFinite(snapshot.position) || snapshot.position < 0) {
    throw new Error("Could not capture the complete playback queue. Audio engine was not changed.");
  }
  if (queueLoudnessContexts !== undefined && (!queueLoudnessContexts || typeof queueLoudnessContexts !== "object"
    || Object.entries(queueLoudnessContexts).some(([entryId, resource]) => !ids.includes(entryId)
      || resource?.scope !== "album" || !/^[a-f0-9]{64}$/i.test(resource.resourceKey)
      || !Array.isArray(resource.members) || resource.members.length < 2 || resource.members.length > 200
      || !Array.isArray(resource.interTrackGapsSeconds) || resource.interTrackGapsSeconds.length + 1 !== resource.members.length
      || resource.interTrackGapsSeconds.some(gap => gap !== 0)))) {
    throw new Error("Could not capture the complete playback queue. Audio engine was not changed.");
  }
  return {
    tracks: [...queue], entryIds: [...ids], currentIndex, repeat: repeat as PlaybackQueueSnapshot["repeat"],
    ...(queueLoudnessContexts ? { queueLoudnessContexts } : {}),
  };
}

/** Read legacy storage before any Native snapshot can overwrite it. SQLite's
 * engine receipt is authoritative; browser markers from earlier builds are not. */
export async function migrateNativePlayback(
  invoke: <T>(command: string, args?: Record<string, unknown>) => Promise<T>,
  storage: Pick<Storage, "getItem">,
  controls: { volume: number; shuffle: boolean },
  captured?: { queue: PlaybackQueueSnapshot; position: number },
): Promise<NativePlaybackSnapshot> {
  // A Native owner must not depend on readable legacy browser storage.
  const current = await invoke<NativePlaybackSnapshot & { migrationRequired?: boolean }>("migrate_native_playback", { legacy: null });
  if (!current.migrationRequired) return current;
  const { loadPlaybackQueue } = await import("./playback-queue");
  const { chapterRelativePosition } = await import("./chapters");
  const rawQueue = captured ? null : storage.getItem("burrata.playback-queue.v1");
  const loaded = captured ? null : loadPlaybackQueue(storage);
  if (rawQueue !== null && !loaded) throw new Error("Saved playback queue is invalid; migration was not committed.");
  const queue = captured?.queue ?? loaded?.snapshot() ?? { tracks: [], entryIds: [], currentIndex: -1, repeat: "off" };
  const track = queue.tracks[queue.currentIndex];
  const entryIds = queue.entryIds ?? [];
  let position = captured ? (track?.playbackRange ? chapterRelativePosition(track,captured.position) : captured.position) : 0;
  const raw = captured ? null : storage.getItem("burrata.playback-position.v1");
  if (raw) {
    try {
      const saved = JSON.parse(raw);
      if (saved.trackId === track?.id && (!saved.entryId || saved.entryId === entryIds[queue.currentIndex]) &&
          Number.isFinite(saved.position) && saved.position >= 0) {
        position = track?.playbackRange ? chapterRelativePosition(track, saved.position) : saved.position;
      }
    } catch { /* A corrupt bookmark does not invalidate a complete queue. */ }
  }
  return invoke<NativePlaybackSnapshot>("migrate_native_playback", { legacy: {
    tracks: queue.tracks, entryIds, index: queue.currentIndex,
    currentEntryId: entryIds[queue.currentIndex] ?? null, queueRevision: 0,
    position, ...controls, repeat: queue.repeat,
    ...(queue.queueLoudnessContexts ? { queueLoudnessContexts: queue.queueLoudnessContexts } : {}),
  } });
}
