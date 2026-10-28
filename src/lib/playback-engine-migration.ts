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
