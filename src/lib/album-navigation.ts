import type { Track } from "./types";
import type { ArtistRef, ArtistReleaseRef } from "./artist-types";

function record(value: unknown): Record<string, unknown> {
  return value && typeof value === "object" && !Array.isArray(value) ? value as Record<string, unknown> : {};
}

/** An exact authenticated search row may restore an older snapshot's album locator.
 * This returns navigation context only; it does not replace track tags or audio identity. */
export function freshSpotifyAlbumContext(original: Track, current: Track): {albumId:string;albumUri:string} | null {
  const id = original.providerId;
  if (original.provider !== "spotify" || current.provider !== "spotify" || !/^[A-Za-z0-9]{22}$/.test(id)
    || original.id !== `spotify:${id}` || current.id !== original.id || current.providerId !== id
    || original.sourceUrl !== `https://open.spotify.com/track/${id}` || current.sourceUrl !== original.sourceUrl
    || original.title !== current.title || original.artist !== current.artist || original.album !== current.album) return null;
  const manual = record(record(original.provenance).manualFields);
  if (["artist","title","album","durationSeconds"].some(key=>manual[key]===true)) return null;
  if (original.durationSeconds != null && (current.durationSeconds == null
    || !Number.isFinite(original.durationSeconds) || !Number.isFinite(current.durationSeconds)
    || original.durationSeconds<=0 || current.durationSeconds<=0
    || Math.abs(original.durationSeconds-current.durationSeconds)>1)) return null;
  const p=record(current.provenance), context=record(p.recordingContext);
  const catalog=record(p.catalog);
  if (catalog.provider!=="spotify" || catalog.verified!==true || catalog.providerId!==id) return null;
  if (typeof context.albumId!=="string" || !/^[A-Za-z0-9]{22}$/.test(context.albumId)
    || context.albumUri!==`spotify:album:${context.albumId}`
    || (p.albumId!==undefined && p.albumId!==context.albumId)) return null;
  return {albumId:context.albumId,albumUri:context.albumUri};
}

/** An album search hint uses provider credit boundaries, never punctuation guesses. */
export function albumLookupArtist(track: Track): string {
  const explicit = track.albumArtist?.trim();
  if (explicit && !/^(?:unknown|untitled|n\/?a|none|not available)$/i.test(explicit)) return explicit;
  const provenance = record(track.provenance);
  const credits = provenance.artistCredits;
  if (Array.isArray(credits) && credits.length && credits.every(value => {
    const credit = record(value);
    return typeof credit.name === "string" && credit.name.trim() && (credit.role == null || typeof credit.role === "string");
  })) {
    const credit = credits.map(record).find(value => value.role == null || !String(value.role).trim()
      || /^(?:artist(?: credit)?|main(?: artist)?|primary(?: artist)?|performer|featured(?: artist)?|vocals?)$/i.test(String(value.role).trim()));
    if (credit) return String(credit.name).trim();
  }
  const catalog = record(provenance.catalog), providerCredits = catalog.artists;
  if (track.provider === "spotify" && catalog.provider === "spotify" && catalog.verified === true
    && Array.isArray(providerCredits) && providerCredits.length > 0 && providerCredits.every(value => {
      const credit = record(value);
      return credit.provider === "spotify" && typeof credit.id === "string" && /^[A-Za-z0-9]{22}$/.test(credit.id)
        && typeof credit.name === "string" && credit.name.trim();
    })) return String(record(providerCredits[0]).name).trim();
  return track.artist.trim();
}

/** Preserve exact catalog release identities instead of starting a new search. */
export function directCatalogAlbumTarget(track: Track): { release: ArtistReleaseRef; artist: ArtistRef } | null {
  if (track.provider !== "catalog") return null;
  const provenance = record(track.provenance), provider = provenance.catalogProvider;
  if (provider !== "musicbrainz" && provider !== "discogs") return null;
  const validId = (value: unknown): value is string => typeof value === "string" && (provider === "musicbrainz"
    ? /^[0-9a-f]{8}-[0-9a-f]{4}-[1-5][0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/i.test(value)
    : /^[1-9][0-9]{0,19}$/.test(value) && BigInt(value) <= 18446744073709551615n);
  if (!validId(provenance.releaseId)) return null;
  const releaseId = provenance.releaseId;
  const sourceUrl = provider === "musicbrainz" ? `https://musicbrainz.org/release/${releaseId}` : `https://www.discogs.com/release/${releaseId}`;
  const credits = (Array.isArray(provenance.artistCredits) ? provenance.artistCredits : []).flatMap(value => {
    const credit = record(value), artist = record(credit.artist);
    if (artist.provider !== provider || !validId(artist.id) || typeof credit.name !== "string" || !credit.name.trim()) return [];
    return [{name: credit.name, artist: {provider, id: artist.id} as ArtistRef,
      role: typeof credit.role === "string" ? credit.role : "artist credit",
      creditedAs: typeof credit.creditedAs === "string" ? credit.creditedAs : null,
      joinPhrase: typeof credit.joinPhrase === "string" ? credit.joinPhrase : null, sourceUrl}];
  });
  const artist = credits.find(credit => /artist|performer/i.test(credit.role))?.artist;
  if (!artist) return null;
  return {artist, release: {release: {provider, id: releaseId}, title: track.album?.trim() || track.title, tracks: [], sourceUrl, credits}};
}
