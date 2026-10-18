/** Keep the requested catalog track ID attached to its files when recording IDs differ. */
export interface TrackFileGroup<T> { trackId: string; files: T[] }

export function trackFileGroupsByTrackId<T>(groups: TrackFileGroup<T>[]): Record<string, T[]> {
  return Object.fromEntries(groups.map(({ trackId, files }) => [trackId, files]));
}
