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
export interface LibraryAlbumTarget { entity: LibraryEntity; familyListingId: string | null }
export interface LibraryEntityGroup { key: string; entries: LibraryEntity[] }
export type LibraryEntityLayout = 'grid' | 'list';
export interface LibraryEntityLayouts { artistLayout: LibraryEntityLayout; albumLayout: LibraryEntityLayout }
const libraryEntityLayoutsKey = 'burrata.library-browser-layouts.v1';
export function readLibraryEntityLayouts(): LibraryEntityLayouts {
  const defaults: LibraryEntityLayouts = { artistLayout: 'grid', albumLayout: 'grid' };
  try {
    if (typeof localStorage === 'undefined') return defaults;
    const value: unknown = JSON.parse(localStorage.getItem(libraryEntityLayoutsKey) ?? 'null');
    if (!value || typeof value !== 'object') return defaults;
    const stored = value as Record<string, unknown>;
    return {
      artistLayout: stored.artistLayout === 'list' ? 'list' : 'grid',
      albumLayout: stored.albumLayout === 'list' ? 'list' : 'grid',
    };
  } catch { return defaults; }
}
export function saveLibraryEntityLayouts(layouts: LibraryEntityLayouts): void {
  try {
    if (typeof localStorage !== 'undefined') localStorage.setItem(libraryEntityLayoutsKey, JSON.stringify({
      artistLayout: layouts.artistLayout === 'list' ? 'list' : 'grid',
      albumLayout: layouts.albumLayout === 'list' ? 'list' : 'grid',
    }));
  } catch { /* Layout preference is optional when browser storage is unavailable. */ }
}
function libraryLabelKey(value: string): string { return value.trim().toLocaleLowerCase().replace(/\s+/g, ' '); }
/** Group matching album titles/artists and artist names for presentation only. Entity IDs remain independent. */
export function groupLibraryEntities(items: readonly LibraryEntity[]): LibraryEntityGroup[] {
  const groups = new Map<string, LibraryEntity[]>();
  for (const item of items) {
    const key = item.kind === 'album'
      ? `album\u0000${libraryLabelKey(item.artist)}\u0000${libraryLabelKey(item.name)}`
      : `${item.kind}\u0000${libraryLabelKey(item.name)}`;
    const group = groups.get(key) ?? [];
    group.push(item);
    groups.set(key, group);
  }
  return [...groups].map(([key, entries]) => ({ key, entries }));
}
export interface LibraryBrowserState {
  tab: 'tracks' | 'artists' | 'albums';
  entity: LibraryEntity | null;
  familyListingId: string | null;
  section: 'albums' | 'tracks';
  query: string;
  sort: string;
  descending: boolean;
  availability: string;
  minDuration: string;
  maxDuration: string;
  entitySource: string;
  entityYear: string;
  entitySort: 'name' | 'artist' | 'year' | 'source' | 'tracks';
  entityDescending: boolean;
  artistLayout: LibraryEntityLayout;
  albumLayout: LibraryEntityLayout;
  scroll: number;
  loadedCount: number;
}
export const initialLibraryBrowserState = (): LibraryBrowserState => ({ tab: 'tracks', entity: null, familyListingId: null, section: 'albums', query: '', sort: 'sourceOrder', descending: false, availability: 'all', minDuration: '', maxDuration: '', entitySource: '', entityYear: '', entitySort: 'name', entityDescending: false, ...readLibraryEntityLayouts(), scroll: 0, loadedCount: 200 });
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
