import type { DetectedTrackObservation } from "./playing-track-detection";
import type { Track } from "./types";

/** Build a stable, searchable standalone track identity from a recognition result. */
export function detectedObservationTrack(observation: DetectedTrackObservation): Track {
  const isrc = observation.identifiers?.isrc?.trim() || null;
  const shazamId = observation.identifiers?.shazamId?.trim() || null;
  const identity = isrc ? `isrc:${isrc.toUpperCase()}` : shazamId ? `shazam:${shazamId}` : `${observation.artist.trim().toLowerCase()}\u0000${observation.title.trim().toLowerCase()}`;
  const providerId = encodeURIComponent(identity);
  return {
    id: `detected:${providerId}`,
    provider: "detected",
    providerId,
    sourceUrl: "",
    title: observation.title,
    artist: observation.artist,
    favorite: false,
    artworkUrl: observation.artworkUrl?.trim() || null,
    isrc,
    provenance: {
      sourceKind: "detected-track",
      shazamId,
      detectedAtSeconds: observation.detectedAtSeconds,
      capturedStartSeconds: observation.capturedStartSeconds,
      capturedEndSeconds: observation.capturedEndSeconds,
      observedAt: observation.observedAt,
    },
  };
}
