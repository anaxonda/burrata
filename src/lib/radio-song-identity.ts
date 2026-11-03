import type { Track } from "./types";

/** Project a canonical radio-song listing ID into storage identity fields. */
export function radioSongTrackIdentity(
  id: string,
): { id: string; provider: "radio_song"; providerId: string } | null {
  const prefix = "radio_song:";
  if (!id.startsWith(prefix)) return null;

  const providerId = id.slice(prefix.length);
  if (!/^[a-f0-9]{64}$/.test(providerId)) return null;

  return { id, provider: "radio_song", providerId };
}

export type RadioSongSnapshot = {
  id: string;
  artist?: unknown;
  title?: unknown;
  track?: unknown;
  metadata?: unknown;
  stationName?: unknown;
};

function record(value: unknown): Record<string, unknown> | null {
  return value && typeof value === "object" && !Array.isArray(value)
    ? value as Record<string, unknown>
    : null;
}

function normalizedIsrc(value: unknown): string | undefined {
  if (typeof value !== "string") return undefined;
  const trimmed = value.trim();
  const code = trimmed.length === 15 && trimmed[2] === "-" && trimmed[6] === "-" && trimmed[9] === "-"
    ? trimmed.replaceAll("-", "").toUpperCase()
    : trimmed.toUpperCase();
  return /^[A-Z]{2}[A-Z0-9]{3}[0-9]{7}$/.test(code) ? code : undefined;
}

/**
 * Project native radio metadata into the canonical song track used for
 * favorites. Never inherit the live station stream track's provenance.
 */
export function radioSongTrackFromSnapshot(radio: RadioSongSnapshot): Track | null {
  const identity = radioSongTrackIdentity(radio.id);
  if (!identity) return null;

  if (radio.track != null) {
    const candidate = record(radio.track);
    const provenance = record(candidate?.provenance);
    const metadata = record(radio.metadata);
    const expectedArtist = typeof metadata?.artist === "string" ? metadata.artist : radio.artist;
    const expectedTitle = typeof metadata?.title === "string" ? metadata.title : radio.title;
    if (
      !candidate ||
      candidate.id !== identity.id ||
      candidate.provider !== identity.provider ||
      candidate.providerId !== identity.providerId ||
      candidate.sourceUrl !== "" ||
      provenance === null ||
      provenance.sourceKind !== "radioSong" ||
      "radioStreamUrl" in provenance ||
      typeof candidate.artist !== "string" ||
      typeof candidate.title !== "string" ||
      (typeof expectedArtist === "string" && candidate.artist.trim() !== expectedArtist.trim()) ||
      (typeof expectedTitle === "string" && candidate.title.trim() !== expectedTitle.trim()) ||
      (typeof metadata?.stationId === "string" && provenance.radioStationId !== metadata.stationId)
    ) return null;
    return candidate as unknown as Track;
  }

  const metadata = record(radio.metadata);
  if (!metadata || typeof metadata.artist !== "string" || typeof metadata.title !== "string") return null;
  const identifiers = record(metadata.identifiers);
  const stationId = typeof metadata.stationId === "string" ? metadata.stationId : "";
  if (!stationId) return null;
  const stationName = typeof radio.stationName === "string" ? radio.stationName : "";
  const artworkUrl = typeof metadata.artworkUrl === "string" ? metadata.artworkUrl : undefined;
  const album = typeof metadata.album === "string" ? metadata.album : undefined;
  const confidence = typeof metadata.metadataConfidence === "string" ? metadata.metadataConfidence : "";
  const observedAt = typeof metadata.observedAt === "string" ? metadata.observedAt : null;
  const recognitionIdentifiers = identifiers ? {
    ...(typeof identifiers.isrc === "string" ? { isrc: identifiers.isrc } : {}),
    ...(typeof identifiers.shazamId === "string" ? { shazamId: identifiers.shazamId } : {}),
    ...(typeof identifiers.appleMusicUrl === "string" ? { appleMusicUrl: identifiers.appleMusicUrl } : {}),
  } : null;
  const isrc = normalizedIsrc(identifiers?.isrc);

  return {
    ...identity,
    sourceUrl: "",
    artist: metadata.artist.trim(),
    title: metadata.title.trim(),
    album,
    artworkUrl,
    isrc,
    favorite: false,
    provenance: {
      sourceKind: "radioSong",
      identity: "unverified station metadata",
      catalogVerified: false,
      radioStationId: stationId,
      radioStationName: stationName,
      radioMetadataConfidence: confidence,
      recognitionIdentifiers,
      radioObservedAt: observedAt,
    },
  };
}
