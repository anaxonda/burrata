import type { Track } from "./types";

export type SearchUiOccurrenceIdentity = {
  sourceId: string;
  rowId: string;
  rowKind: string;
  track: Pick<Track, "id" | "provider" | "providerId" | "sourceUrl" | "title" | "artist" | "version" | "durationSeconds" | "playbackRange">;
};

/** Identity of the currently loaded playable SearchPanel snapshot. Playback position is deliberately excluded. */
export function searchUiControlFingerprint(input: {
  text: string; submitted: string; mode: string; source: string; kind: string;
  sort: string; direction: string; minimumDuration: string; maximumDuration: string;
  generation: number; collectionRequest: number; continueSearch: boolean;
  occurrences: SearchUiOccurrenceIdentity[];
}): string {
  return JSON.stringify([
    input.text, input.submitted, input.mode, input.source, input.kind,
    input.sort, input.direction, input.minimumDuration, input.maximumDuration,
    input.generation, input.collectionRequest, input.continueSearch,
    input.occurrences.map(({ sourceId, rowId, rowKind, track }) => [
      sourceId, rowId, rowKind, track.id, track.provider, track.providerId,
      track.sourceUrl, track.title, track.artist, track.version, track.durationSeconds,
      track.playbackRange ? [track.playbackRange.parentTrackId, track.playbackRange.chapterId,
        track.playbackRange.sourceUrl, track.playbackRange.startTime, track.playbackRange.endTime,
        track.playbackRange.parentTitle, track.playbackRange.parentDurationSeconds] : null,
    ]),
  ]);
}

export function searchUiControlExpired(expiresAtMs: number, nowMs: number): boolean {
  return !Number.isSafeInteger(expiresAtMs) || !Number.isSafeInteger(nowMs) || nowMs >= expiresAtMs;
}
