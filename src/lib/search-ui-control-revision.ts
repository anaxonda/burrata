import type { Track } from "./types";

export type SearchUiOccurrenceIdentity = {
  sourceId: string;
  rowId: string;
  rowKind: string;
  track: Pick<Track, "id" | "provider" | "providerId" | "sourceUrl" | "title" | "artist" | "version" | "isrc" | "durationSeconds" | "playbackRange">;
};

type SearchUiControlSnapshot = {
  controls: string;
  groups: Map<string, { start: number; occurrences: SearchUiControlOccurrence[] }>;
};

type SearchUiControlOccurrence = {
  identity: string;
  duration: number | null | [shortest: number, longest: number];
};

function searchUiControlDuration(track: SearchUiOccurrenceIdentity["track"]): SearchUiControlOccurrence["duration"] {
  const seconds = track.durationSeconds ?? null;
  if (track.provider === "youtube" && !track.playbackRange
    && /^[A-Za-z0-9_-]{11}$/.test(track.providerId) && track.id === `youtube:${track.providerId}`
    && track.sourceUrl === `https://www.youtube.com/watch?v=${track.providerId}`
    && seconds !== null && Number.isFinite(seconds) && seconds > 0) {
    return [seconds, seconds];
  }
  return seconds;
}

/** Identity of the playable SearchPanel snapshot. Playback position and artwork are excluded. */
export function captureSearchUiControlSnapshot(input: {
  text: string; submitted: string; mode: string; source: string; kind: string;
  sort: string; direction: string; minimumDuration: string; maximumDuration: string;
  downloadAvailability?: "all" | "downloaded" | "not-downloaded";
  generation: number; collectionRequest: number; continueSearch: boolean;
  visibleSources: string[];
  occurrences: SearchUiOccurrenceIdentity[];
}): SearchUiControlSnapshot {
  const controls = JSON.stringify([
    input.text, input.submitted, input.mode, input.source, input.kind,
    input.sort, input.direction, input.minimumDuration, input.maximumDuration, input.downloadAvailability ?? "all",
    input.generation, input.collectionRequest, input.continueSearch, input.visibleSources,
  ]);
  const groups: SearchUiControlSnapshot["groups"] = new Map();
  for (const [index, { sourceId, rowId, rowKind, track }] of input.occurrences.entries()) {
    let group = groups.get(sourceId);
    if (!group) {
      group = { start: index, occurrences: [] };
      groups.set(sourceId, group);
    }
    group.occurrences.push({
      identity: JSON.stringify([
        rowId, rowKind, track.id, track.provider, track.providerId,
        track.sourceUrl, track.title, track.artist, track.version, track.isrc,
        track.playbackRange ? [track.playbackRange.parentTrackId, track.playbackRange.chapterId,
          track.playbackRange.sourceUrl, track.playbackRange.startTime, track.playbackRange.endTime,
          track.playbackRange.parentTitle, track.playbackRange.parentDurationSeconds] : null,
      ]),
      duration: searchUiControlDuration(track),
    });
  }
  return { controls, groups };
}

/** Keep provider-local positions and reconcile safe metadata refinements within the same revision. */
export function extendSearchUiControlSnapshot(previous: SearchUiControlSnapshot, next: SearchUiControlSnapshot): SearchUiControlSnapshot | null {
  if (previous.controls !== next.controls) return null;
  const groups = new Map(next.groups);
  for (const [sourceId, group] of previous.groups) {
    const current = next.groups.get(sourceId);
    if (!current || current.occurrences.length < group.occurrences.length) return null;
    const occurrences = [...current.occurrences];
    for (const [index, before] of group.occurrences.entries()) {
      const after = occurrences[index];
      if (before.identity !== after.identity) return null;
      if (Object.is(before.duration, after.duration)) continue;
      if (!Array.isArray(before.duration) || !Array.isArray(after.duration)) return null;
      // Bound the total observed spread so consecutive refreshes can't drift.
      const shortest = Math.min(before.duration[0], after.duration[0]);
      const longest = Math.max(before.duration[1], after.duration[1]);
      if (longest - shortest > 1) return null;
      occurrences[index] = { ...after, duration: [shortest, longest] };
    }
    groups.set(sourceId, { ...current, occurrences });
  }
  return { ...next, groups };
}

export function searchUiControlExpired(expiresAtMs: number, nowMs: number): boolean {
  return !Number.isSafeInteger(expiresAtMs) || !Number.isSafeInteger(nowMs) || nowMs >= expiresAtMs;
}
