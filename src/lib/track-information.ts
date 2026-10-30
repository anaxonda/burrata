import type { ArtistProfile, ArtistProvider, ArtistRef, ArtistSearchPage } from "./artist-types";
import type { Track } from "./types";

export interface TrackArtistCredit {
  name: string;
  role: string;
  artist: ArtistRef | null;
}

export interface TrackArtistInformation {
  /** Credit names as supplied by canonical provenance, in credit order. */
  artistCredits: TrackArtistCredit[];
  primaryArtist: { name: string; artist: ArtistRef };
  profile: ArtistProfile;
}

export interface TrackArtistInformationLoaderOptions {
  searchArtists: (name: string) => Promise<ArtistSearchPage>;
  getArtistProfile: (artist: ArtistRef) => Promise<ArtistProfile>;
  /** Optional read-only recording lookup for tracks without a direct artist ref. */
  lookupSpotifyArtist?: (id: string) => Promise<ArtistRef | null>;
  lookupRecordingArtist?: (track: Track, artistName: string) => Promise<ArtistRef | null>;
  positiveCacheMs?: number;
  negativeCacheMs?: number;
  maxCacheEntries?: number;
  now?: () => number;
}

export interface TrackArtistInformationLoader {
  load(track: Track): Promise<TrackArtistInformation | null>;
  clear(): void;
}

type RawCredit = { name?: unknown; role?: unknown; artist?: unknown };
type RawCatalogArtist = { name?: unknown; provider?: unknown; id?: unknown };
type ResolvedArtist = { artist: ArtistRef; profile: ArtistProfile };
type CacheValue = { expiresAt: number; value: Promise<ResolvedArtist | null> };

const DEFAULT_POSITIVE_CACHE_MS = 10 * 60_000;
const DEFAULT_NEGATIVE_CACHE_MS = 15_000;
const DEFAULT_MAX_CACHE_ENTRIES = 128;

function record(value: unknown): Record<string, unknown> {
  return value && typeof value === "object" && !Array.isArray(value) ? value as Record<string, unknown> : {};
}

function isArtistRef(value: unknown): value is ArtistRef {
  const raw = record(value);
  if (raw.provider === "musicbrainz") return typeof raw.id === "string" && /^[0-9a-f]{8}-[0-9a-f]{4}-[1-5][0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/i.test(raw.id);
  if (raw.provider === "discogs") return typeof raw.id === "string" && /^[1-9][0-9]*$/.test(raw.id);
  if (raw.provider === "monochrome") return typeof raw.id === "string" && /^[A-Za-z0-9_-]{1,80}$/.test(raw.id);
  return false;
}

function normalizedName(value: string): string {
  return value.normalize("NFKC").trim().replace(/\s+/g, " ").toLocaleLowerCase();
}

function provenanceCredits(track: Track): TrackArtistCredit[] {
  const provenance = record(track.provenance);
  const rawCredits = Array.isArray(provenance.artistCredits) ? provenance.artistCredits as RawCredit[] : [];
  const credits: TrackArtistCredit[] = rawCredits.flatMap(raw => {
    const name = typeof raw?.name === "string" ? raw.name.trim() : "";
    if (!name) return [];
    return [{ name, role: typeof raw.role === "string" && raw.role.trim() ? raw.role.trim() : "artist", artist: isArtistRef(raw.artist) ? { ...raw.artist } : null }];
  });

  const catalog = record(provenance.catalog);
  const rawCatalog = Array.isArray(catalog.artists) ? catalog.artists as RawCatalogArtist[] : [];
  for (const raw of rawCatalog) {
    const name = typeof raw?.name === "string" ? raw.name.trim() : "";
    if (!name) continue;
    const ref = isArtistRef({ provider: raw.provider, id: raw.id }) ? { provider: raw.provider as ArtistProvider, id: raw.id as string } : null;
    const existing = credits.find(credit => normalizedName(credit.name) === normalizedName(name));
    if (existing) {
      if (!existing.artist && ref) existing.artist = ref;
      continue;
    }
    credits.push({ name, role: "artist", artist: ref });
  }

  // Legacy tracks often have only one raw artist string. Keep it intact; commas,
  // ampersands and other join phrases are not reliable credit boundaries.
  if (!credits.length && typeof track.artist === "string" && track.artist.trim()) {
    credits.push({ name: track.artist.trim(), role: "artist", artist: null });
  }
  return credits;
}

function primaryCredit(credits: TrackArtistCredit[]): TrackArtistCredit | null {
  return credits.find(credit => /artist/i.test(credit.role)) ?? credits[0] ?? null;
}

function chooseExactArtist(page: ArtistSearchPage, name: string): ArtistRef | null {
  const wanted = normalizedName(name);
  const byProvider = new Map<ArtistProvider, Map<string, ArtistRef>>();
  for (const result of page.results ?? []) {
    if (!result || typeof result.name !== "string" || normalizedName(result.name) !== wanted || !isArtistRef(result.artist)) continue;
    const ref = result.artist;
    let entries = byProvider.get(ref.provider);
    if (!entries) { entries = new Map(); byProvider.set(ref.provider, entries); }
    entries.set(ref.id, ref);
  }
  for (const provider of ["musicbrainz", "discogs", "monochrome"] as const) {
    const matches = byProvider.get(provider);
    if (matches && matches.size > 1) return null;
    if (matches?.size === 1) return [...matches.values()][0]!;
  }
  return null;
}

/** Creates a shared read-only artist profile loader for track detail surfaces. */
export function createTrackArtistInformationLoader(options: TrackArtistInformationLoaderOptions): TrackArtistInformationLoader {
  const cache = new Map<string, CacheValue>();
  const now = options.now ?? Date.now;
  const positiveTtl = Math.max(0, options.positiveCacheMs ?? DEFAULT_POSITIVE_CACHE_MS);
  const negativeTtl = Math.max(0, options.negativeCacheMs ?? DEFAULT_NEGATIVE_CACHE_MS);
  const maxEntries = Math.max(1, Math.floor(options.maxCacheEntries ?? DEFAULT_MAX_CACHE_ENTRIES));

  async function resolve(track: Track, primary: TrackArtistCredit): Promise<ResolvedArtist | null> {
    let artist = primary.artist;
    if (!artist && options.lookupSpotifyArtist) {
      const catalog = record(record(track.provenance).catalog);
      const entries = Array.isArray(catalog.artists) ? catalog.artists : [];
      const matching = entries.filter((entry: unknown) => {
        const item = record(entry);
        return item.provider === "spotify" && typeof item.id === "string" && /^[A-Za-z0-9]{22}$/.test(item.id) && typeof item.name === "string" && normalizedName(item.name) === normalizedName(primary.name);
      });
      if (catalog.verified === true && catalog.provider === "spotify" && matching.length === 1) artist = await options.lookupSpotifyArtist(record(matching[0]).id as string);
    }
    if (!artist) {
      const page = await options.searchArtists(primary.name);
      artist = chooseExactArtist(page, primary.name);
      if (!artist && options.lookupRecordingArtist) artist = await options.lookupRecordingArtist(track, primary.name);
    }
    if (!artist || !isArtistRef(artist)) return null;
    const profile = await options.getArtistProfile(artist);
    return { artist, profile };
  }

  return {
    async load(track) {
      const credits = provenanceCredits(track);
      const primary = primaryCredit(credits);
      if (!primary) return null;
      const key = primary.artist ? `${primary.artist.provider}:${primary.artist.id}` : `name:${normalizedName(primary.name)}:track:${track.id}:${track.isrc ?? ""}:${normalizedName(track.title ?? "")}`;
      const existing = cache.get(key);
      if (existing && existing.expiresAt > now()) {
        cache.delete(key); cache.set(key, existing);
        const result = await existing.value;
        return result ? { artistCredits: credits, primaryArtist: { name: primary.name, artist: result.artist }, profile: result.profile } : null;
      }
      if (existing) cache.delete(key);
      const value = resolve(track, primary).catch(error => {
        cache.delete(key);
        throw error;
      });
      cache.set(key, { expiresAt: now() + (positiveTtl || negativeTtl), value });
      while (cache.size > maxEntries) cache.delete(cache.keys().next().value!);
      try {
        const result = await value;
        const entry = cache.get(key);
        if (entry?.value === value) entry.expiresAt = now() + (result ? positiveTtl : negativeTtl);
        return result ? { artistCredits: credits, primaryArtist: { name: primary.name, artist: result.artist }, profile: result.profile } : null;
      } catch (error) {
        cache.delete(key);
        throw error;
      }
    },
    clear() { cache.clear(); },
  };
}
