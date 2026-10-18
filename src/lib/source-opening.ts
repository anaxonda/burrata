import type { Track } from "./types";
export const SOURCE_OPENER = "burrata.source-opener";
export type SourceOpener = (track: Track, url: string) => Promise<void>;
/** Positions are relative to the playable track, including chapter excerpts. */
export function sourcePositionUrl(raw: string, track: Track, position: number): string {
  const url = new URL(raw);
  const seconds = Math.max(0, Math.floor((Number.isFinite(position) ? position : 0) + (track.playbackRange?.startTime ?? 0)));
  const host = url.hostname.toLowerCase();
  if (host === "youtu.be" || host === "youtube.com" || host.endsWith(".youtube.com")) {
    url.searchParams.set("t", `${seconds}s`);
  } else if (host === "soundcloud.com" || host.endsWith(".soundcloud.com")) {
    url.hash = `t=${Math.floor(seconds / 60)}:${String(seconds % 60).padStart(2, "0")}`;
  }
  return url.href;
}
