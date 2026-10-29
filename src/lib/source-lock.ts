import type { Candidate, Track } from "./types";

const CATALOG_PROVIDERS = new Set([
  "catalog", "spotify", "monochrome", "musicbrainz", "discogs", "beatport", "traxsource",
]);

function provenanceOf(track: Track): Record<string, unknown> {
  return track.provenance && typeof track.provenance === "object"
    ? track.provenance as Record<string, unknown>
    : {};
}

/** Whether the row represents a catalog recording identity rather than an exact audio URL. */
export function isCatalogSelection(track: Track): boolean {
  const provenance = provenanceOf(track);
  const catalogProvider = typeof provenance.catalogProvider === "string" ? provenance.catalogProvider : "";
  const catalog = provenance.catalog && typeof provenance.catalog === "object"
    ? provenance.catalog as Record<string, unknown> : {};
  return CATALOG_PROVIDERS.has(track.provider) || CATALOG_PROVIDERS.has(catalogProvider) ||
    CATALOG_PROVIDERS.has(String(catalog.provider ?? ""));
}

/**
 * True only when playback must remain tied to a specific source or file.
 * A candidate cached for a catalog result remains replaceable after transport
 * failure, so the resolver can try another source for the same recording.
 */
export function hasExplicitSourceLock(
  track: Track,
  options: { selectedCandidate?: Candidate | null; audioSearchResult?: boolean } = {},
): boolean {
  const provenance = provenanceOf(track);
  const sourceKind = String(provenance.sourceKind ?? "").toLowerCase();
  const catalogSelection = isCatalogSelection(track);
  if (track.playbackRange || track.localPath || track.provider === "radio" ||
      provenance.userSelectedSource === true || provenance.explicitSourceLock === true ||
      ["youtuberadio", "upload", "uploaded", "localupload"].includes(sourceKind)) return true;

  if (options.selectedCandidate && !isCatalogSelection(track)) return true;

  const playbackMode = provenance.playbackMode;
  if (playbackMode === "source") return true;
  if (playbackMode === "lookup") return false;

  const audioCandidate = provenance.audioCandidate && typeof provenance.audioCandidate === "object"
    ? provenance.audioCandidate as Record<string, unknown> : {};
  const embeddedExactVideoSource = ["youtube", "soundcloud"].includes(String(audioCandidate.provider));
  if ((sourceKind === "audiosearch" || options.audioSearchResult === true) &&
      (!catalogSelection || embeddedExactVideoSource)) return true;

  if (["youtube", "soundcloud"].includes(track.provider)) return true;

  // A selected candidate on an ordinary/import row is an explicit source pick.
  // Catalog identities retain fallback eligibility even when a source candidate
  // was cached alongside the recording.
  return Boolean(options.selectedCandidate && !catalogSelection);
}
