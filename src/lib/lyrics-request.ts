import type { Track, NativePlaybackSnapshot } from "./types";

export interface VisibleLyricsTrack {
  trackId: string;
  localPath: string | null;
  trackName: string;
  artistName: string;
  albumName: string | null;
  durationSeconds: number;
  playbackRange: Track["playbackRange"] | null;
  radio?: boolean;
  detection?: { trackId: string; generation: number; seekSerial: number; resourceKey: string; detectedAtSeconds: number };
}

/** Snapshot the player item metadata used for WebView lyrics lookup and translation. */
export function visibleLyricsTrack(track: Track | null | undefined, detection?: VisibleLyricsTrack["detection"]): VisibleLyricsTrack | undefined {
  if (!track) return undefined;
  const metadata=(track.provenance as {radioMetadata?:{title?:unknown;artist?:unknown}}|null)?.radioMetadata;
  const radio=["radio","radioparadise","internet-radio"].includes(track.provider) && typeof metadata?.title === "string" && metadata.title.trim() && metadata.title===track.title && typeof metadata.artist === "string" && metadata.artist.trim() && metadata.artist===track.artist;
  return {
    trackId: detection?.trackId ?? track.id,
    localPath: track.localPath ?? null,
    trackName: track.title,
    artistName: track.artist,
    albumName: track.album ?? null,
    durationSeconds: track.durationSeconds ?? 0,
    playbackRange: track.playbackRange ?? null,
    ...(radio ? {radio:true,durationSeconds:0} : {}),
    ...(detection ? {detection, durationSeconds:0, playbackRange:null} : {}),
  };
}

/** Playback-only duration projection; never promotes decoder observations into saved metadata. */
export function lyricsTrackWithPlaybackDuration(track: Track | null, snapshot: Pick<NativePlaybackSnapshot, "engine" | "trackId" | "duration"> | null): Track | null {
  const valid = (duration: unknown): duration is number => typeof duration === "number" && Number.isFinite(duration) && duration >= 1 && duration <= 3600;
  if (!track || ["radio","radioparadise","internet-radio"].includes(track.provider) || valid(track.durationSeconds) || track.playbackRange || (track.provenance as Record<string, unknown> | null)?.approximateExcerpt || snapshot?.engine !== "native" || snapshot.trackId !== track.id || !valid(snapshot.duration)) return track;
  return {...track, durationSeconds: snapshot.duration};
}

/** Lyrics for a detected set chapter use the chapter-local playback clock. */
export function lyricsPlaybackPosition(track: Track | null | undefined, absolutePosition: number): number {
  const detectedAt = (track?.provenance as {lyricsDetectedAtSeconds?:unknown}|null)?.lyricsDetectedAtSeconds;
  if (typeof detectedAt === "number" && Number.isFinite(detectedAt)) return Math.max(0, absolutePosition - detectedAt);
  const start = track?.playbackRange?.startTime;
  return typeof start === "number" && Number.isFinite(start) ? Math.max(0, absolutePosition - start) : absolutePosition;
}
