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
  options: { selectedCandidate?: Candidate | null; audioSearchResult?: boolean; overridePlaylistTrackUrls?: boolean; localPathPins?: boolean } = {},
): boolean {
  const provenance = provenanceOf(track);
  const sourceKind = String(provenance.sourceKind ?? "").toLowerCase();
  // A registered file may win local playback without selecting its remembered
  // online candidate. Native candidate forwarding opts out of that file-only pin.
  if (track.playbackRange || (options.localPathPins !== false && track.localPath) || ["radio", "local", "upload", "slsk", "slskd"].includes(track.provider) ||
      provenance.userSelectedSource === true || provenance.explicitSourceLock === true ||
      ["youtuberadio", "upload", "uploaded", "localupload"].includes(sourceKind)) return true;

  const playbackMode = provenance.playbackMode;
  const manualFields = provenance.manualFields && typeof provenance.manualFields === "object"
    ? provenance.manualFields as Record<string, unknown> : {};
  const automaticBrowserSource = provenance.browserImport === true && provenance.captureProvider === "youtube" &&
    playbackMode === "source" && manualFields.playbackMode !== true;
  if (playbackMode === "source" && !automaticBrowserSource) return true;
  if (playbackMode === "lookup") return false;

  if (options.selectedCandidate) return true;

  if (track.provider === "import" && options.overridePlaylistTrackUrls === true) return false;

  return false;
}
