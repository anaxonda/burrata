export interface TrackLibrarySummary {
  trackId: string;
  playCount: number;
  /** RFC 3339 timestamps in newest-first order. */
  playedAt: string[];
  /** True when older playback dates exist beyond the returned recent-date window. */
  playedAtHasMore?: boolean;
  playlists: Array<{ id: string; name: string }>;
  inLibrary: boolean;
  downloaded: boolean;
}
