import type { TrackSortAvailability } from "./track-sort-availability";
export const LOCAL_TRACK_PAGE_SIZE = 200;

export interface LocalTrackPage<T> {
  tracks: T[];
  total: number;
  sortPending?: boolean;
  sortAvailability?: TrackSortAvailability | null;
  sortAvailabilityPending?: boolean;
}

export type LocalTrackPageResult<T> =
  | { kind: "stale" }
  | { kind: "invalid" }
  | { kind: "sortPending"; page: LocalTrackPage<T> }
  | { kind: "ready"; page: LocalTrackPage<T> };

/** Dispatches the full library page and rejects replies from an older query generation. */
export async function requestLocalTrackPage<T>(
  dispatch: (args: Record<string, unknown>) => Promise<LocalTrackPage<T> | null>,
  options: Record<string, unknown>,
  offset: number,
  isCurrent: () => boolean,
): Promise<LocalTrackPageResult<T>> {
  const page = await dispatch({ ...options, limit: LOCAL_TRACK_PAGE_SIZE, offset });
  if (!isCurrent()) return { kind: "stale" };
  if (!page || !Array.isArray(page.tracks)) return { kind: "invalid" };
  if (page.sortPending === true) return { kind: "sortPending", page };
  return { kind: "ready", page };
}

export type AppliedLocalTrackPage<T> =
  | { kind: "sortPending"; tracks: T[]; total: number; loadMoreDisabled: true }
  | { kind: "ready"; tracks: T[]; total: number; loadMoreDisabled: false };

/** A pending sort response intentionally leaves the prior rows and total in place. */
export function applyLocalTrackPage<T>(
  currentTracks: T[],
  currentTotal: number,
  page: LocalTrackPage<T> | null,
  append: boolean,
): AppliedLocalTrackPage<T> {
  if (page?.sortPending === true) {
    return { kind: "sortPending", tracks: currentTracks, total: currentTotal, loadMoreDisabled: true };
  }
  if (!page || !Array.isArray(page.tracks)) {
    return { kind: "ready", tracks: currentTracks, total: currentTotal, loadMoreDisabled: false };
  }
  return {
    kind: "ready",
    tracks: append ? [...currentTracks, ...page.tracks] : page.tracks,
    total: page.total,
    loadMoreDisabled: false,
  };
}
