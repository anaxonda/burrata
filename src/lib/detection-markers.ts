import type { DetectedTrackObservation } from "./playing-track-detection";

export interface DetectionMarker {
  startSeconds: number;
  endSeconds: number;
  title: string;
  colorIndex: number;
}

function songColorIndex(artist: string, title: string): number {
  const identity = `${artist.trim().toLocaleLowerCase()}\u0000${title.trim().toLocaleLowerCase()}`;
  let hash = 2166136261;
  for (let index = 0; index < identity.length; index++) {
    hash ^= identity.charCodeAt(index);
    hash = Math.imul(hash, 16777619);
  }
  return (hash >>> 0) % 6;
}

/** Map approximate detected-song capture windows onto the current playback axis. */
export function detectionMarkers(
  observations: DetectedTrackObservation[],
  durationSeconds: number,
  sourceOffsetSeconds = 0,
): DetectionMarker[] {
  if (!Number.isFinite(durationSeconds) || durationSeconds <= 0 || !Number.isFinite(sourceOffsetSeconds)) return [];
  return observations.flatMap((observation) => {
    const startSeconds = Math.max(0, observation.capturedStartSeconds - sourceOffsetSeconds);
    const capturedDuration = observation.capturedEndSeconds - observation.capturedStartSeconds;
    const confirmedEnd = Number.isFinite(observation.lastConfirmedAtSeconds)
      ? observation.lastConfirmedAtSeconds! + Math.max(0, capturedDuration) / 2
      : observation.capturedEndSeconds;
    // The row stores the first and latest confirmation points. Extend the
    // first captured window only through the estimated end of the last
    // confirmed sample; never infer an entire song beyond those observations.
    const endSeconds = Math.min(durationSeconds, Math.max(observation.capturedEndSeconds, confirmedEnd) - sourceOffsetSeconds);
    if (!Number.isFinite(startSeconds) || !Number.isFinite(endSeconds) || endSeconds <= startSeconds) return [];
    return [{ startSeconds, endSeconds, title: `${observation.artist} — ${observation.title}`, colorIndex: songColorIndex(observation.artist, observation.title) }];
  });
}
