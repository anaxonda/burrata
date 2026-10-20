import type { Track } from "./types";

const mbid = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i;

export function musicBrainzCover(sourceUrl: unknown): string | null {
  if (typeof sourceUrl !== "string") return null;
  try {
    const url = new URL(sourceUrl);
    const [, kind, id, ...rest] = url.pathname.split("/");
    if (url.protocol !== "https:" || url.hostname !== "musicbrainz.org" || url.username || url.password || url.port || url.search || url.hash || rest.length || !["release", "release-group"].includes(kind) || !mbid.test(id)) return null;
    return `https://coverartarchive.org/${kind}/${id}/front-250`;
  } catch { return null; }
}

export function trackArtworkSources(track: Track): (string | null | undefined)[] {
  const provenance = track.provenance as Record<string, unknown> | null | undefined;
  const group = provenance?.catalogProvider === "musicbrainz" && typeof provenance.releaseGroupId === "string" && mbid.test(provenance.releaseGroupId)
    ? `https://coverartarchive.org/release-group/${provenance.releaseGroupId}/front-250` : null;
  return [track.artworkUrl, group, musicBrainzCover(provenance?.requestedReleaseSourceUrl)];
}
