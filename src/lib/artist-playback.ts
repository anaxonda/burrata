import type { LocalAsset, Track } from './types';
const normalized = (value: string | null | undefined) => (value ?? '').normalize('NFKC').trim().replace(/\s+/g, ' ').toLocaleLowerCase();
/** Reuse unique, available local audio without merging catalog recording identities. */
export function artistPlaybackTracks(tracks: Track[], assets: LocalAsset[]): Track[] {
  return tracks.map(track => {
    if (track.localAvailability === 'available' && track.localPath) return track;
    const matches = assets.filter(asset => asset.track.localAvailability === 'available'
      && normalized(asset.track.artist) === normalized(track.artist)
      && normalized(asset.track.title) === normalized(track.title)
      && normalized(asset.track.version) === normalized(track.version)
      && (!track.durationSeconds || !asset.track.durationSeconds || Math.abs(track.durationSeconds - asset.track.durationSeconds) <= 3));
    if (matches.length !== 1) return track;
    const asset = matches[0];
    return {...track, localPath: asset.path, assetId: asset.id, localAvailability: 'available',
      provenance: {...(track.provenance && typeof track.provenance === 'object' ? track.provenance : {}), localPlaybackMatch: 'unique exact artist, title and version'}};
  });
}
