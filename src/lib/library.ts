import type { Track } from './types';
import type { ArtistReleaseRef } from './artist-types';

export interface LibraryEntity {
  id: string;
  kind: 'artist' | 'album';
  name: string;
  artist: string;
  provider?: string | null;
  providerId?: string | null;
  sourceUrl?: string | null;
  artworkUrl?: string | null;
  releaseDate?: string | null;
  totalTracks?: number | null;
  explicitlySaved: boolean;
  trackCount: number;
  albumCount: number;
  aliases?: string[];
}
export interface LibraryEntityPage { items: LibraryEntity[]; total: number }
export interface LibraryBrowserState {
  tab: 'tracks' | 'artists' | 'albums';
  entity: LibraryEntity | null;
  section: 'albums' | 'tracks';
  query: string;
  sort: string;
  descending: boolean;
  availability: string;
  minDuration: string;
  maxDuration: string;
  scroll: number;
  loadedCount: number;
}
export const initialLibraryBrowserState = (): LibraryBrowserState => ({ tab: 'tracks', entity: null, section: 'albums', query: '', sort: 'sourceOrder', descending: false, availability: 'all', minDuration: '', maxDuration: '', scroll: 0, loadedCount: 200 });
export function releaseLibraryEntity(release: ArtistReleaseRef, artist = ''): LibraryEntity {
  return { id: '', kind: 'album', name: release.title, artist: release.credits.map(c => c.name).join(', ') || artist,
    provider: release.release.provider, providerId: release.release.id, sourceUrl: release.sourceUrl,
    artworkUrl: release.artworkUrls?.[0], releaseDate: release.releaseDate ?? (release.year ? String(release.year) : null), totalTracks: release.trackCount,
    explicitlySaved: false, trackCount: 0, albumCount: 0 };
}
export function completeLibraryAlbumTrackCount(tracks: Track[], reportedCount?: number | null): number | null {
  if (!tracks.length) return null;
  const snapshots = tracks.map(track => {
    const provenance = track.provenance && typeof track.provenance === 'object' ? track.provenance as Record<string, unknown> : {};
    return provenance.libraryAlbumSnapshot as { complete?: boolean; total?: number } | null | undefined;
  });
  if (snapshots.some(Boolean)) {
    return snapshots.every(snapshot => snapshot?.complete === true && snapshot.total === tracks.length) ? tracks.length : null;
  }
  return Number.isInteger(reportedCount) && reportedCount === tracks.length ? tracks.length : null;
}
export async function saveCompleteLibraryAlbum(
  call: <T = unknown>(command: string, args?: Record<string, unknown>) => Promise<T>,
  entry: LibraryEntity, tracks: Track[], expectedCount: number | null | undefined,
): Promise<void> {
  const total = completeLibraryAlbumTrackCount(tracks, expectedCount);
  if (total === null) {
    throw new Error('The complete album tracklist is required. Retry loading the album or save loaded tracks instead.');
  }
  await call('save_library_album', { entry: { ...entry, totalTracks: total }, tracks, expectedCount: total });
  window.dispatchEvent(new Event('library-changed'));
}
