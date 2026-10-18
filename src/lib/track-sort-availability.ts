import { hasComparableTrackSortValue, type TrackSortField } from "./track-sort";
import type { Track } from "./types";

export type TrackSortAvailability = Readonly<Record<TrackSortField, boolean>>;

export type TrackSortSource = string;

// Fields the source adapter can return for these source views. Unknown sources
// stay value-driven, so an incomplete capability table never disables a field.
const SOURCE_SORT_FIELDS: Readonly<Record<string, ReadonlySet<TrackSortField>>> = {
  // This is an operation-specific parser contract, not a provider-wide claim.
  // Last.fm artist top tracks construct only title and artist on Track.
  "lastfm-top-tracks": new Set(["sourceOrder", "title", "artist"]),
};
const SOURCE_LABELS: Readonly<Record<string, string>> = { "lastfm-top-tracks": "Last.fm top tracks" };

const TRACK_SORT_FIELDS: readonly TrackSortField[] = [
  "title", "artist", "album", "duration", "releaseDate", "dateAdded", "bpm", "key", "plays", "views",
];

/** Source order is always meaningful. Determine the other fields once per dataset. */
export function trackSortAvailability(tracks: readonly Track[]): TrackSortAvailability {
  const available = Object.fromEntries(TRACK_SORT_FIELDS.map(field => [field, false])) as Record<TrackSortField, boolean>;
  available.sourceOrder = true;
  for (const track of tracks) {
    for (const field of TRACK_SORT_FIELDS) {
      if (!available[field] && hasComparableTrackSortValue(track, field)) available[field] = true;
    }
    if (TRACK_SORT_FIELDS.every(field => available[field])) break;
  }
  return available;
}

/** When a local page omits rows from the active query, its unseen values are unknown. */
export function allTrackSortFieldsAvailable(): TrackSortAvailability {
  return { sourceOrder: true, title: true, artist: true, album: true, duration: true, releaseDate: true, dateAdded: true, bpm: true, key: true, plays: true, views: true };
}

/**
 * Combines observed values with source capability. Incomplete result sets keep
 * known-supported and unknown fields available while paging; only a field every
 * selected source explicitly lacks is disabled as unsupported.
 */
export function trackSortAvailabilityForSources(
  tracks: readonly Track[],
  sources: readonly TrackSortSource[],
  incomplete = false,
): TrackSortAvailability {
  const observedAvailability = trackSortAvailability(tracks);
  const availability: Record<TrackSortField, boolean> = { ...(incomplete ? allTrackSortFieldsAvailable() : observedAvailability) };
  if (!sources.length) return availability;
  const sourceSupport = sources.map(source => SOURCE_SORT_FIELDS[source]);
  for (const field of TRACK_SORT_FIELDS) {
    const observed = observedAvailability[field];
    if (sourceSupport.every(fields => fields?.has(field) === false) && !observed) availability[field] = false;
    else if (incomplete || observed || sourceSupport.some(fields => fields?.has(field))) availability[field] = true;
  }
  return availability;
}

/** Keep an unavailable selected option understandable when the dataset changes. */
export function trackSortOptionLabel(availability: TrackSortAvailability, field: TrackSortField, label: string): string {
  return availability[field] ? label : `${label} (unavailable)`;
}

/** Make known source limitations distinct from fields absent in this dataset. */
export function trackSortOptionLabelForSources(
  availability: TrackSortAvailability,
  field: TrackSortField,
  label: string,
  sources: readonly TrackSortSource[],
): string {
  const unsupported = !availability[field] && sources.length > 0 && sources.every(source => SOURCE_SORT_FIELDS[source]?.has(field) === false);
  if (unsupported) {
    const names = [...new Set(sources.map(source => SOURCE_LABELS[source] ?? source))];
    return `${label} (not supported by ${names.join(" or ")})`;
  }
  return trackSortOptionLabel(availability, field, label);
}
