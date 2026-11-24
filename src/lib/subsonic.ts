import type { Track } from "./types";

export type SubsonicStatus = {
  endpoint: string | null;
  username: string | null;
  visible: boolean;
  allowFallback: boolean;
  trackCount: number;
  canSeed: boolean;
  strawberryFound: boolean;
  refreshAfterLogin?: boolean;
  warning?: string | null;
};

export type SubsonicPage = { tracks: Track[]; total: number; snapshotId?: string | null };

export type SubsonicScope = { kind: "artist" | "album"; trackId: string; label: string; provider?: "subsonic" | "jellyfin" };

export function serverLibraryEntityId(track: Track, kind: "artist" | "album"): string | null {
  if (track.provider !== "subsonic" && track.provider !== "jellyfin") return null;
  const account = /^([a-f0-9]{64})\./.exec(track.providerId)?.[1];
  const provenance = track.provenance as { serverArtistId?: unknown; subsonicArtistId?: unknown; serverAlbumId?: unknown; subsonicAlbumId?: unknown; manualFields?: { artist?: boolean } } | undefined;
  const id = kind === "artist" ? provenance?.serverArtistId ?? provenance?.subsonicArtistId : provenance?.serverAlbumId ?? provenance?.subsonicAlbumId;
  if (!account || (kind === "artist" && provenance?.manualFields?.artist) || typeof id !== "string" || !id || id.length > 512) return null;
  const encoded = btoa(Array.from(new TextEncoder().encode(id), byte => String.fromCharCode(byte)).join(""))
    .replace(/\+/g, "-").replace(/\//g, "_").replace(/=+$/, "");
  return `${kind}:ref:${track.provider}:${account}.${encoded}`;
}

export function subsonicTrackArtworkKey(track: Track): string {
  const provenance = track.provenance as { serverCoverArtId?: unknown; serverAlbumId?: unknown; subsonicCoverArtId?: unknown; subsonicAlbumId?: unknown } | undefined;
  const account = track.id.match(/^(?:subsonic|jellyfin):([a-f0-9]{64})\./)?.[1];
  const cover = provenance?.serverCoverArtId ?? provenance?.subsonicCoverArtId;
  const album = provenance?.serverAlbumId ?? provenance?.subsonicAlbumId;
  if (account && typeof cover === "string") {
    return `${track.provider}-cover:${account}:${cover}`;
  }
  if (account && typeof album === "string") {
    return `${track.provider}-album-cover:${account}:${album}`;
  }
  return `server-track-cover:${track.id}`;
}

/** Validate raster thumbnails returned by Burrata's local Subsonic command. */
export function subsonicArtworkDataUrl(value: unknown): string | null {
  return typeof value === "string" && value.length <= 524320
    && /^data:image\/(?:jpeg|png|webp);base64,[A-Za-z0-9+/]+={0,2}$/.test(value)
    ? value : null;
}
