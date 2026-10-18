import type { Track } from "./types";

export type TrackSortField =
  | "title"
  | "artist"
  | "album"
  | "duration"
  | "releaseDate"
  | "dateAdded"
  | "sourceOrder"
  | "bpm"
  | "key"
  | "plays"
  | "views";
export type SortDirection = "asc" | "desc";
export const TRACK_SORT_LOCALE = "en-US";
const TRACK_TEXT_COLLATOR = new Intl.Collator(TRACK_SORT_LOCALE, {
  usage: "sort",
  sensitivity: "base",
  numeric: true,
});

export function compareTrackText(left: string, right: string): number {
  return TRACK_TEXT_COLLATOR.compare(left, right);
}

export function normalizeTrackText(value: string): string {
  return value.toLowerCase();
}
export interface TrackFilter {
  query?: string;
  minDurationSeconds?: number;
  maxDurationSeconds?: number;
  minBpm?: number;
  maxBpm?: number;
}

/** Duration to display and play for a track, using the chapter's own range when present. */
export function playableDuration(track: Track): number | null {
  const range = track.playbackRange;
  const seconds = range ? range.endTime - range.startTime : track.durationSeconds;
  return finite(seconds) && seconds > 0 ? seconds : null;
}

/** Parse duration text as seconds or mm:ss; invalid or negative values return null. */
export function parseDurationBound(input: string | number | null | undefined): number | null {
  if(input===null||input===undefined)return null;
  const value = String(input).trim();
  if (!value) return null;
  const parts = value.split(":");
  let seconds: number;
  if (parts.length === 1) seconds = Number(parts[0]);
  else if (parts.length === 2 && /^\d+$/.test(parts[0]) && /^\d{1,2}(?:\.\d+)?$/.test(parts[1])) {
    const tail = Number(parts[1]);
    if (tail >= 60) return null;
    seconds = Number(parts[0]) * 60 + tail;
  } else return null;
  return Number.isFinite(seconds) && seconds >= 0 ? Math.floor(seconds) : null;
}

/** Apply the shared library text and numeric filters while preserving every matching occurrence. */
export function filterTracks<T extends Track>(tracks: readonly T[], filter: TrackFilter): T[] {
  const query = filter.query ? normalizeTrackText(filter.query.trim()) : undefined;
  return tracks.filter((track) => {
    if (query) {
      const haystack = [track.title, track.artist, track.album, track.version, track.genre]
        .filter((value): value is string => typeof value === "string")
        .join(" ").toLowerCase();
      if (!haystack.includes(query)) return false;
    }
    const seconds = wholeDuration(track);
    if (filter.minDurationSeconds !== undefined && (seconds === null || seconds < filter.minDurationSeconds)) return false;
    if (filter.maxDurationSeconds !== undefined && (seconds === null || seconds > filter.maxDurationSeconds)) return false;
    if (filter.minBpm !== undefined && (!finite(track.bpm) || track.bpm <= 0 || track.bpm < filter.minBpm)) return false;
    if (filter.maxBpm !== undefined && (!finite(track.bpm) || track.bpm <= 0 || track.bpm > filter.maxBpm)) return false;
    return true;
  });
}

type Sortable = { track: Track; index: number };

/** Sort track occurrences without mutating the input. Missing values always follow known values. */
export function sortTracks<T extends Track>(
  tracks: readonly T[],
  field: TrackSortField,
  direction: SortDirection = "asc",
): T[] {
  const sign = direction === "desc" ? -1 : 1;
  return tracks
    .map((track, index): Sortable & { track: T } => ({ track, index }))
    .sort((a, b) => {
      const av = valueFor(a.track, field, a.index);
      const bv = valueFor(b.track, field, b.index);
      if (av === null) return bv === null ? a.index - b.index : 1;
      if (bv === null) return -1;
      const cmp = typeof av === "number" && typeof bv === "number"
        ? av - bv
        : compareTrackText(String(av), String(bv));
      return cmp === 0 ? a.index - b.index : cmp * sign;
    })
    .map(({ track }) => track);
}

function valueFor(track: Track, field: TrackSortField, index: number): string | number | null {
  const provenance = track.provenance && typeof track.provenance === "object"
    ? track.provenance as Record<string, unknown>
    : {};
  switch (field) {
    case "title": return text(track.title);
    case "artist": return text(track.artist);
    case "album": return text(track.album);
    case "duration": {
      return wholeDuration(track);
    }
    case "releaseDate": return normalizeSortDate(track.releaseDate ?? track.recordingDate);
    case "dateAdded": return normalizeSortDate(track.dateAdded ?? first(provenance, ["dateAdded", "addedAt", "importedAt", "createdAt"]));
    case "sourceOrder": return index;
    case "bpm": return finite(track.bpm) && track.bpm > 0 ? track.bpm : null;
    case "key": return keyValue(track.key);
    case "plays": return normalizeSortCount(first(provenance, ["plays", "playCount", "play_count"])) ?? normalizeSortCount(first(nested(provenance, "remoteCatalog"), ["plays", "playCount", "play_count"]));
    case "views": return normalizeSortCount(first(provenance, ["views", "viewCount", "view_count"])) ?? normalizeSortCount(first(nested(provenance, "remoteCatalog"), ["views", "viewCount", "view_count"]));
  }
}

/** Whether this track contributes a real comparison value for the selected field. */
export function hasComparableTrackSortValue(track: Track, field: TrackSortField): boolean {
  return field === "sourceOrder" || valueFor(track, field, 0) !== null;
}

function wholeDuration(track: Track): number | null {
  const seconds = playableDuration(track);
  return seconds === null ? null : Math.floor(seconds);
}

function text(value: unknown): string | null {
  return typeof value === "string" && value.trim() ? value.trim() : null;
}
function finite(value: unknown): value is number { return typeof value === "number" && Number.isFinite(value) && value >= 0; }
export function normalizeSortCount(value: unknown): number | null {
  if (finite(value)) return value;
  if (typeof value === "string") {
    const match = value.trim().replace(/,/g, "").match(/^(\d+(?:\.\d+)?)\s*([kmb])?\s*(?:plays?|views?)?$/i);
    if (match) {
      const multiplier = { k: 1_000, m: 1_000_000, b: 1_000_000_000 }[match[2]?.toLowerCase() as "k" | "m" | "b"] ?? 1;
      const parsed = Number(match[1]) * multiplier;
      return Number.isFinite(parsed) ? parsed : null;
    }
  }
  return null;
}
function first(obj: Record<string, unknown>, keys: string[]): unknown {
  for (const key of keys) if (obj[key] !== undefined && obj[key] !== null) return obj[key];
  return null;
}
function nested(obj: Record<string, unknown>, key: string): Record<string, unknown> {
  const value = obj[key];
  return value && typeof value === "object" && !Array.isArray(value) ? value as Record<string, unknown> : {};
}
export function normalizeSortDate(value: unknown): number | null {
  if (typeof value !== "string") return null;
  const input = value.trim();
  const partial = input.match(/^(\d{4})(?:-(\d{2})(?:-(\d{2}))?)?$/);
  if (partial) {
    const year = Number(partial[1]);
    const month = Number(partial[2] ?? 1);
    const day = Number(partial[3] ?? 1);
    const date = new Date(0);
    date.setUTCHours(0, 0, 0, 0);
    date.setUTCFullYear(year, month - 1, day);
    if (date.getUTCFullYear() !== year || date.getUTCMonth() !== month - 1 || date.getUTCDate() !== day) return null;
    return date.getTime();
  }
  const full = input.match(/^(\d{4})-(\d{2})-(\d{2})T(\d{2}):(\d{2}):(\d{2})(?:\.(\d{1,9}))?(Z|[+-]\d{2}:\d{2})$/);
  if (!full || Number(full[4]) > 23 || Number(full[5]) > 59 || Number(full[6]) > 59) return null;
  const year = Number(full[1]), month = Number(full[2]), day = Number(full[3]);
  const date = new Date(0);
  date.setUTCFullYear(year, month - 1, day);
  if (date.getUTCFullYear() !== year || date.getUTCMonth() !== month - 1 || date.getUTCDate() !== day) return null;
  if (full[8] !== "Z" && (Number(full[8].slice(1, 3)) > 23 || Number(full[8].slice(4, 6)) > 59)) return null;
  const timestamp = Date.parse(input);
  return Number.isFinite(timestamp) ? timestamp : null;
}

const PITCH: Record<string, number> = {
  c: 0, "b#": 0, "c#": 1, db: 1, d: 2, "d#": 3, eb: 3, e: 4, "fb": 4,
  "e#": 5, f: 5, "f#": 6, gb: 6, g: 7, "g#": 8, ab: 8, a: 9,
  "a#": 10, bb: 10, b: 11, cb: 11,
};
function keyValue(value: unknown): number | null {
  if (typeof value !== "string" || !value.trim()) return null;
  // Match Rust's key normalizer: trim/remove ASCII spaces only, then accept canonical Camelot numbers.
  const normalized = value.replace(/^ +| +$/g, "").toLowerCase()
    .replace(/♯/g, "#").replace(/♭/g, "b").replace(/ /g, "");
  const camelot = normalized.match(/^(\d{1,2})([ab])$/);
  if (camelot) {
    const number = Number(camelot[1]);
    if (number < 1 || number > 12 || String(number) !== camelot[1]) return null;
    const minorPitch = (number * 7 + 1) % 12;
    return camelot[2] === "a" ? minorPitch * 2 + 1 : ((minorPitch + 3) % 12) * 2;
  }
  const match = normalized.match(/^([a-g](?:#|b)?)(major|minor|maj|min|m)?$/);
  if (!match) return null;
  const pitch = PITCH[match[1]];
  if (pitch === undefined) return null;
  const mode = match[2] === "minor" || match[2] === "min" || match[2] === "m" ? 1 : 0;
  return pitch * 2 + mode;
}
