import type { Track, Candidate } from "./types";

const exactCountFormat = new Intl.NumberFormat();
const compactCountFormat = new Intl.NumberFormat(undefined, { notation: "compact" });

export function trackProviderCount(track: Track): { label: string; full: string } | null {
  const spotify = track.provider === "spotify";
  if (!spotify && track.provider !== "youtube") return null;
  const count = metadataCount(track, null, spotify
    ? ["plays", "playCount", "play_count", "playcount"]
    : ["views", "viewCount", "view_count"]);
  if (count === null) {
    if (spotify) return null;
    const provenance = track.provenance as { remoteCatalog?: { viewCountText?: unknown } } | null;
    const label = provenance?.remoteCatalog?.viewCountText;
    return typeof label === "string" && label.trim()
      ? { label, full: `${label} on YouTube` }
      : null;
  }
  if (!Number.isSafeInteger(count)) return null;
  const unit = spotify ? (count === 1 ? "play" : "plays") : (count === 1 ? "view" : "views");
  const source = spotify ? "Spotify" : "YouTube";
  return {
    label: `${compactCountFormat.format(count)} ${unit}`,
    full: `${exactCountFormat.format(count)} ${source} ${unit}`,
  };
}

export function metadataCount(track:Track,candidate:Candidate|null|undefined,keys: string[]): number | null {
    const provenance = track.provenance && typeof track.provenance === "object" ? track.provenance as Record<string, unknown> : {};
    const sourceMetadata = candidate?.sourceMetadata && typeof candidate.sourceMetadata === "object" ? candidate.sourceMetadata : {};
    const provenanceRemote = provenance.remoteCatalog && typeof provenance.remoteCatalog === "object" ? provenance.remoteCatalog as Record<string, unknown> : {};
    const sourceRemote = sourceMetadata.remoteCatalog && typeof sourceMetadata.remoteCatalog === "object" ? sourceMetadata.remoteCatalog as Record<string, unknown> : {};
    for (const source of [sourceMetadata, sourceRemote, provenanceRemote, provenance]) {
      for (const key of keys) {
        const value = source[key];
        if (typeof value === "number" && Number.isFinite(value) && value >= 0) return value;
        if (typeof value === "string" && /^\s*\d[\d,]*(?:\.\d+)?\s*$/.test(value)) {
          const parsed = Number(value.replaceAll(",", "").trim());
          if (Number.isFinite(parsed) && parsed >= 0) return parsed;
        }
      }
    }
    return null;
  }
