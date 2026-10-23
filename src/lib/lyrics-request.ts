import type { Track } from "./types";

export interface VisibleLyricsTrack {
  trackId: string;
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
    trackName: track.title,
    artistName: track.artist,
    albumName: track.album ?? null,
    durationSeconds: track.durationSeconds ?? 0,
    playbackRange: track.playbackRange ?? null,
  };
}
