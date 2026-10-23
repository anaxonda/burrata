import type { Track } from "./types";

export interface VisibleLyricsTrack {
  trackId: string;
  localPath: string | null;
  trackName: string;
  artistName: string;
  albumName: string | null;
  durationSeconds: number;
  playbackRange: Track["playbackRange"] | null;
}

/** Snapshot the player item metadata used for WebView lyrics lookup and translation. */
export function visibleLyricsTrack(track: Track | null | undefined): VisibleLyricsTrack | undefined {
  if (!track) return undefined;
  return {
    trackId: track.id,
    localPath: track.localPath ?? null,
    trackName: track.title,
    artistName: track.artist,
    albumName: track.album ?? null,
    durationSeconds: track.durationSeconds ?? 0,
    playbackRange: track.playbackRange ?? null,
  };
}
