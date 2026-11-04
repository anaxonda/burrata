export interface TrackLibrarySummary {
  trackId: string;
  playCount: number;
  /** RFC 3339 timestamps in newest-first order. */
  playedAt: string[];
  /** Stable playback-history session ID paired with each displayed timestamp. */
  playedOccurrences?: Array<{ sessionId: string; playedAt: string }>;
  /** True when older playback dates exist beyond the returned recent-date window. */
  playedAtHasMore?: boolean;
  playlists: Array<{ id: string; name: string }>;
  inLibrary: boolean;
  downloaded: boolean;
}

export interface PlaylistMembershipBinding {
  collectionId: string;
  sourceCollectionId?: string | null;
}

/**
 * Hide the remote snapshot membership only when its linked local collection is
 * also in the track summary and both collections still have the same name.
 * The local collection is the playable working copy; same-name collections
 * without an explicit binding remain distinct playlists.
 */
export function projectPlaylistMembershipsForDisplay<T extends { id: string; name: string }>(
  playlists: readonly T[],
  bindings: readonly PlaylistMembershipBinding[],
): T[] {
  const byId = new Map(playlists.map(playlist => [playlist.id, playlist]));
  const workingCopyIds = new Set(bindings.map(binding => binding.collectionId));
  const hiddenSourceIds = new Set<string>();

  for (const binding of bindings) {
    const sourceId = binding.sourceCollectionId;
    // If a collection is also the destination of another binding, its role is
    // ambiguous (for example, a chain or cycle). Keep it visible as a working
    // copy rather than hiding it as a source snapshot.
    if (!sourceId || sourceId === binding.collectionId || workingCopyIds.has(sourceId)) continue;
    const source = byId.get(sourceId);
    const workingCopy = byId.get(binding.collectionId);
    if (source && workingCopy && source.name === workingCopy.name) hiddenSourceIds.add(sourceId);
  }

  return hiddenSourceIds.size ? playlists.filter(playlist => !hiddenSourceIds.has(playlist.id)) : [...playlists];
}
