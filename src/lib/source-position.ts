import type { Track } from "./types";

/** Playlist order remains separate from the provider's chart ranking. */
export function playlistPosition(track: Track): number | null {
  if (!track.provenance || typeof track.provenance !== "object") return null;
  const value = (track.provenance as Record<string, unknown>).playlistPosition;
  return typeof value === "number" && Number.isInteger(value) && value > 0
    ? value
    : null;
}
export function sourcePosition(track: Track): number | null {
  return track.rank ?? playlistPosition(track);
}
