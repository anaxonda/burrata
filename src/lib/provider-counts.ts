import type { Track, Candidate } from "./types";

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
