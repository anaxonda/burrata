import type { ArtistCredit, ArtistRef } from "./artist-types";
import type { Track } from "./types";

export interface RecordingSearchRequest {
  query: string;
  sourceIds: string[];
  kinds: string[];
  limit: number;
  isrc?: string;
}

export interface RecordingSearchRow {
  id: string;
  kind?: string;
  sourceId?: string;
  title?: string;
  subtitle?: string | null;
  track?: Track | null;
  data?: unknown;
}

export interface RecordingSearchPage {
  results?: RecordingSearchRow[];
}

export interface RecordingReleaseInfo {
  provider: "musicbrainz";
  id: string;
  title: string;
  date: string | null;
  country: string | null;
  sourceUrl: string;
  /** Present only when the release result itself supplied artist credits. */
  credits?: ArtistCredit[];
}

export interface RecordingCatalogInfo {
  provider: "musicbrainz";
  id: string;
  title: string;
  artist: string | null;
  version: string | null;
  firstReleaseDate: string | null;
  durationSeconds: number | null;
  isrcs: string[];
  genres: string[];
  sourceUrl: string;
  releases: RecordingReleaseInfo[];
  /** Exact recording-level artist credit identity, distinct from release credits. */
  recordingArtist: { name: string; artist: ArtistRef } | null;
}

export interface RecordingInformationCandidate {
  recording: RecordingCatalogInfo;
  identity: "exact_recording_id" | "exact_isrc" | "text_suggestion";
}

export interface RecordingInformationResult {
  resolution: "exact_recording_id" | "exact_isrc" | "ambiguous_isrc" | "unresolved";
  recording: RecordingCatalogInfo | null;
  candidates: RecordingInformationCandidate[];
}

export interface RecordingInformationLoaderOptions {
  searchSource: (request: RecordingSearchRequest) => Promise<RecordingSearchPage>;
  positiveCacheMs?: number;
  negativeCacheMs?: number;
  maxCacheEntries?: number;
  now?: () => number;
}

export interface RecordingInformationLoader {
  load(track: Track): Promise<RecordingInformationResult>;
  clear(): void;
}

type Obj = Record<string, unknown>;
type CacheEntry = { expiresAt: number; promise: Promise<RecordingSearchRow[]> };
const DEFAULT_POSITIVE_TTL_MS = 5 * 60_000;
const DEFAULT_NEGATIVE_TTL_MS = 20_000;
const DEFAULT_CACHE_SIZE = 64;
const MBID = /^[0-9a-f]{8}-[0-9a-f]{4}-[1-5][0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/i;

function object(value: unknown): Obj { return value && typeof value === "object" && !Array.isArray(value) ? value as Obj : {}; }
function text(value: unknown): string | null { return typeof value === "string" && value.trim() ? value.trim() : null; }
function validMbid(value: unknown): value is string { return typeof value === "string" && MBID.test(value); }
function releaseDate(value: unknown): string | null {
  const date = text(value);
  if (!date || date.length > 10 || !/^\d{4}(?:-\d{2}(?:-\d{2})?)?$/.test(date)) return null;
  const [, yearText, monthText, dayText] = /^(\d{4})(?:-(\d{2})(?:-(\d{2}))?)?$/.exec(date)!;
  if (monthText) {
    const month = Number(monthText);
    if (month < 1 || month > 12) return null;
    if (dayText) {
      const day = Number(dayText);
      const year = Number(yearText);
      const maximum = new Date(Date.UTC(year, month, 0)).getUTCDate();
      if (day < 1 || day > maximum) return null;
    }
  }
  return date;
}
function releaseCountry(value: unknown): string | null {
  const country = text(value);
  return country && /^[A-Za-z]{2}$/.test(country) ? country.toUpperCase() : null;
}

export function normalizeRecordingIsrc(value: unknown): string | null {
  if (typeof value !== "string") return null;
  const trimmed = value.trim();
  const normalized = (trimmed.length === 15 && trimmed[2] === "-" && trimmed[6] === "-" && trimmed[9] === "-"
    ? trimmed.replaceAll("-", "") : trimmed).toUpperCase();
  return /^[A-Z]{2}[A-Z0-9]{3}[0-9]{7}$/.test(normalized) ? normalized : null;
}

function normalizeText(value: string): string {
  return value.normalize("NFKC").toLocaleLowerCase().replace(/[\p{P}\p{S}\s]+/gu, "").trim();
}

function recordingIdFromTrack(track: Track): string | null {
  const provenance = object(track.provenance);
  const catalog = object(provenance.catalog);
  const values = [
    track.provider === "musicbrainz" ? track.providerId : null,
    provenance.recordingId,
    catalog.recordingId,
    object(provenance.remoteCatalog).recordingId,
  ];
  return values.find(validMbid) ?? null;
}

function isrcFromTrack(track: Track): string | null {
  const provenance = object(track.provenance);
  const catalog = object(provenance.catalog);
  const remoteCatalog = object(provenance.remoteCatalog);
  const context = object(provenance.recordingContext);
  const recognition = object(provenance.recognitionIdentifiers);
  for (const candidate of [track.isrc, context.isrc, recognition.isrc, remoteCatalog.isrc, catalog.isrc]) {
    const normalized = normalizeRecordingIsrc(candidate);
    if (normalized) return normalized;
  }
  return null;
}

function rawIsrcs(data: Obj): string[] {
  const values = Array.isArray(data.isrcs) ? data.isrcs : [];
  return [...new Set(values.map(normalizeRecordingIsrc).filter((value): value is string => Boolean(value)))];
}

function parseArtistCredits(value: unknown, sourceUrl: string): ArtistCredit[] | undefined {
  if (!Array.isArray(value)) return undefined;
  const credits: ArtistCredit[] = [];
  for (const item of value.slice(0, 32)) {
    const raw = object(item);
    const artistRaw = object(raw.artist);
    const id = artistRaw.id;
    const name = text(raw.name) ?? text(artistRaw.name);
    if (!name || !validMbid(id)) continue;
    credits.push({
      name,
      artist: { provider: "musicbrainz", id },
      role: "artist credit",
      creditedAs: text(raw.name),
      joinPhrase: text(raw.joinphrase),
      sourceUrl,
    });
  }
  return credits.length ? credits : undefined;
}

function parseReleases(value: unknown): RecordingReleaseInfo[] {
  if (!Array.isArray(value)) return [];
  const releases: RecordingReleaseInfo[] = [];
  const seen = new Set<string>();
  for (const item of value.slice(0, 100)) {
    const raw = object(item);
    const id = raw.id;
    const title = text(raw.title);
    if (!validMbid(id) || !title || title.length > 512 || seen.has(id)) continue;
    seen.add(id);
    const sourceUrl = `https://musicbrainz.org/release/${id}`;
    const credits = parseArtistCredits(raw["artist-credit"] ?? raw.artistCredit, sourceUrl);
    releases.push({
      provider: "musicbrainz", id, title,
      date: releaseDate(raw.date),
      country: releaseCountry(raw.country),
      sourceUrl,
      ...(credits ? { credits } : {}),
    });
  }
  return releases;
}

function parseRecording(row: RecordingSearchRow): RecordingCatalogInfo | null {
  const data = object(row.data);
  const id = data.id;
  const title = text(data.title) ?? text(row.track?.title) ?? text(row.title);
  if (!validMbid(id) || !title) return null;
  const sourceUrl = `https://musicbrainz.org/recording/${id}`;
  const credits = parseArtistCredits(data["artist-credit"], sourceUrl) ?? [];
  const firstCredit = credits[0];
  const length = typeof data.length === "number" && Number.isFinite(data.length) && data.length > 0
    ? data.length / 1000 : null;
  const genreValues = Array.isArray(data.genres) ? data.genres : [];
  const genres = [...new Set(genreValues.flatMap(value => {
    const genre = text(object(value).name);
    return genre ? [genre] : [];
  }))].slice(0, 32);
  return {
    provider: "musicbrainz", id, title,
    artist: text(row.track?.artist) ?? (credits.length ? credits.map(credit => `${credit.name}${credit.joinPhrase ?? ""}`).join("") : null),
    version: text(data.disambiguation) ?? text(row.track?.version),
    firstReleaseDate: text(data["first-release-date"]) ?? text(row.track?.releaseDate),
    durationSeconds: length ?? (typeof row.track?.durationSeconds === "number" && Number.isFinite(row.track.durationSeconds) ? row.track.durationSeconds : null),
    isrcs: rawIsrcs(data), genres, sourceUrl,
    releases: parseReleases(data.releases),
    recordingArtist: firstCredit?.artist ? { name: firstCredit.name, artist: firstCredit.artist } : null,
  };
}

function hasContradictoryText(track: Track, recording: RecordingCatalogInfo): boolean {
  const trackTitle = normalizeText(track.title);
  const recordingTitle = normalizeText(recording.title);
  if (trackTitle && recordingTitle && trackTitle !== recordingTitle) return true;
  const expectedVersion = text(track.version);
  const foundVersion = text(recording.version);
  return Boolean(expectedVersion && foundVersion && normalizeText(expectedVersion) !== normalizeText(foundVersion));
}

function echoedRowId(row: RecordingSearchRow, id: string): boolean {
  const dataId = object(row.data).id;
  return dataId === id && (row.id === id || row.id === `musicbrainz:${id}`);
}

function classify(track: Track, rows: RecordingSearchRow[]): RecordingInformationResult {
  const recordingId = recordingIdFromTrack(track);
  const isrc = isrcFromTrack(track);
  if (recordingId) {
    const row = rows.find(item => echoedRowId(item, recordingId));
    const recording = row ? parseRecording(row) : null;
    if (recording && !hasContradictoryText(track, recording)) {
      return { resolution: "exact_recording_id", recording, candidates: [{ recording, identity: "exact_recording_id" }] };
    }
    const suggestions = rows.flatMap(item => { const candidate = parseRecording(item); return candidate ? [{ recording: candidate, identity: "text_suggestion" as const }] : []; });
    return { resolution: "unresolved", recording: null, candidates: suggestions };
  }
  if (isrc) {
    const matches = rows.flatMap(row => {
      const recording = parseRecording(row);
      if (!recording || !recording.isrcs.includes(isrc) || hasContradictoryText(track, recording)) return [];
      return [{ recording, identity: "exact_isrc" as const }];
    });
    const unique = [...new Map(matches.map(candidate => [candidate.recording.id, candidate])).values()];
    if (unique.length === 1) return { resolution: "exact_isrc", recording: unique[0]!.recording, candidates: unique };
    if (unique.length > 1) return { resolution: "ambiguous_isrc", recording: null, candidates: unique };
  }
  const suggestions = rows.flatMap(row => {
    const recording = parseRecording(row);
    return recording ? [{ recording, identity: "text_suggestion" as const }] : [];
  });
  return { resolution: "unresolved", recording: null, candidates: suggestions };
}

/** Fetches bounded MusicBrainz row details for info views; it never mutates tags or playback. */
export function createRecordingInformationLoader(options: RecordingInformationLoaderOptions): RecordingInformationLoader {
  const cache = new Map<string, CacheEntry>();
  const now = options.now ?? Date.now;
  const positiveTtl = Math.max(0, options.positiveCacheMs ?? DEFAULT_POSITIVE_TTL_MS);
  const negativeTtl = Math.max(0, options.negativeCacheMs ?? DEFAULT_NEGATIVE_TTL_MS);
  const maxEntries = Math.max(1, Math.floor(options.maxCacheEntries ?? DEFAULT_CACHE_SIZE));

  async function rowsFor(track: Track): Promise<{ key: string; rows: RecordingSearchRow[] }> {
    const recordingId = recordingIdFromTrack(track);
    const isrc = isrcFromTrack(track);
    const query = `${track.artist ?? ""} ${track.title ?? ""}`.trim().slice(0, 512);
    const key = recordingId ? `recording:${recordingId}` : isrc ? `isrc:${isrc}` : `text:${normalizeText(query)}`;
    const cached = cache.get(key);
    if (cached && cached.expiresAt > now()) {
      cache.delete(key); cache.set(key, cached);
      return { key, rows: await cached.promise };
    }
    if (cached) cache.delete(key);
    let entry: CacheEntry | undefined;
    const promise = options.searchSource({
      query,
      sourceIds: ["musicbrainz"],
      kinds: ["track"],
      limit: 50,
      ...(isrc ? { isrc } : {}),
    }).then(page => Array.isArray(page.results) ? page.results.filter(row => row && row.sourceId !== "discogs" && row.kind !== "artist") : []).catch(error => {
      if (entry && cache.get(key) === entry) cache.delete(key);
      throw error;
    });
    entry = { expiresAt: now() + Math.max(positiveTtl, negativeTtl), promise };
    cache.set(key, entry);
    while (cache.size > maxEntries) cache.delete(cache.keys().next().value!);
    try {
      const rows = await promise;
      return { key, rows };
    } catch (error) {
      if (cache.get(key) === entry) cache.delete(key);
      throw error;
    }
  }

  return {
    async load(track) {
      const { key, rows } = await rowsFor(track);
      const result = classify(track, rows);
      const entry = cache.get(key);
      if (entry) entry.expiresAt = now() + (result.recording ? positiveTtl : negativeTtl);
      return result;
    },
    clear() { cache.clear(); },
  };
}
