import type { Track } from "./types";

/**
 * Returns the canonical identity used when a recording is shown in more than
 * one list. Chapter rows share their own canonical track id, but their source
 * range is also part of that identity so a parent recording or another range
 * can never inherit their playback state.
 */
export function trackIdentity(track: Track): string {
  const range = track.playbackRange;
  if (!range) return JSON.stringify([track.id, "recording"]);
  return JSON.stringify([
    track.id,
    "chapter",
    range.parentTrackId,
    range.chapterId,
    range.sourceUrl,
    range.startTime,
    range.endTime,
  ]);
}

export function sameTrackIdentity(left: Track, right: Track): boolean {
  return trackIdentity(left) === trackIdentity(right);
}
