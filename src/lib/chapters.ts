import type { Candidate, Chapter, Track } from "./types";

export interface PlaybackRange {
  parentTrackId: string;
  chapterId: string;
  sourceUrl: string;
  startTime: number;
  endTime: number;
  parentTitle: string;
  parentDurationSeconds?: number | null;
}

export type RangedTrack = Track & { playbackRange?: PlaybackRange; inferredArtist?: string | null };

export function validChapterRange(chapter: Pick<Chapter, "startTime" | "endTime">, duration?: number | null): boolean {
  return Number.isFinite(chapter.startTime) && Number.isFinite(chapter.endTime) && chapter.startTime >= 0 && chapter.endTime > chapter.startTime && (!Number.isFinite(duration) || duration == null || chapter.endTime <= duration);
}

export function chapterPlaybackRange(parent: Track, candidate: Candidate, chapter: Chapter): PlaybackRange | null {
  if (!validChapterRange(chapter, candidate.durationSeconds)) return null;
  return {
    parentTrackId: parent.id, chapterId: chapter.id, sourceUrl: candidate.url,
    startTime: chapter.startTime, endTime: chapter.endTime, parentTitle: candidate.title || parent.title,
    parentDurationSeconds: candidate.durationSeconds ?? null,
  };
}

/** Creates a queueable chapter view while retaining parent source provenance and avoiding fake catalog identifiers. */
export function virtualChapterTrack(parent: Track, candidate: Candidate, chapter: Chapter): RangedTrack | null {
  const playbackRange = chapterPlaybackRange(parent, candidate, chapter);
  if (!playbackRange) return null;
  const suggested = chapter.suggestedTrack;
  const labelParts = (chapter.title || "").split(/\s+[—–-]\s+/);
  const inferredArtist = suggested?.artist && suggested.artist !== "Unknown artist"
    ? suggested.artist
    : labelParts.length > 1 && labelParts[0]?.trim() && labelParts[1]?.trim() ? labelParts[0].trim() : null;
  const inferredTitle = labelParts.length > 1 && inferredArtist ? labelParts.slice(1).join(" - ").trim() : chapter.title;
  const providerId = `${parent.id}:${chapter.id}`;
  return {
    ...parent,
    id: `chapter:${providerId}`,
    provider: "chapter",
    providerId,
    sourceUrl: candidate.url,
    title: inferredTitle || `Chapter ${chapter.id}`,
    artist: inferredArtist || "",
    version: null,
    album: null,
    label: null,
    genre: null,
    normalizedGenre: null,
    durationSeconds: chapter.endTime - chapter.startTime,
    artworkUrl: candidate.artworkUrl || parent.artworkUrl,
    releaseDate: null,
    isrc: null,
    catalogNumber: null,
    albumArtist: null,
    recordingDate: null,
    trackNumber: null,
    trackTotal: null,
    discNumber: null,
    discTotal: null,
    remixers: null,
    bpm: null,
    key: null,
    playbackRange,
    inferredArtist,
    provenance: {
      sourceKind: "chapter",
      parentTrack: parent,
      parentProvenance: parent.provenance,
      audioCandidate: candidate,
      parentTrackId: parent.id,
      chapterId: chapter.id,
      inferredArtist,
    },
  } as RangedTrack;
}

export function chapterAbsolutePosition(track: Track, relativePosition: number): number {
  const range = (track as RangedTrack).playbackRange;
  if (!range) return Math.max(0, relativePosition);
  return range.startTime + clamp(relativePosition, 0, range.endTime - range.startTime);
}

export function chapterRelativePosition(track: Track, absolutePosition: number): number {
  const range = (track as RangedTrack).playbackRange;
  if (!range) return Math.max(0, absolutePosition);
  return clamp(absolutePosition - range.startTime, 0, range.endTime - range.startTime);
}

export function activeChapter(chapters: readonly Chapter[], position: number): Chapter | null {
  if (!Number.isFinite(position)) return null;
  return chapters.find((chapter) => validChapterRange(chapter) && position >= chapter.startTime && position < chapter.endTime) ?? null;
}

function clamp(value: number, low: number, high: number): number {
  return Math.max(low, Math.min(high, Number.isFinite(value) ? value : low));
}
