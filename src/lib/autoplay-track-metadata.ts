import type { Track } from "./types";

function videoId(track: Track): string | null {
  if (track.provider !== "youtube" || !/^[A-Za-z0-9_-]{11}$/.test(track.providerId) || track.id !== `youtube:${track.providerId}`) return null;
  try {
    const url = new URL(track.sourceUrl);
    if (url.protocol !== "https:" || url.username || url.password) return null;
    const id = ["youtube.com", "www.youtube.com", "music.youtube.com"].includes(url.hostname)
      ? url.searchParams.get("v") : url.hostname === "youtu.be" ? url.pathname.slice(1) : null;
    return id === track.providerId ? id : null;
  } catch { return null; }
}

/** Fill an autoplay snapshot's missing display metadata from the same saved video.
 * Keep its queue identity and source binding: saving metadata never changes the upload.
 */
export function autoplayTrackWithLibraryMetadata(track: Track, library: readonly Track[]): Track {
  if ((track.provenance as Record<string, unknown> | null)?.sourceKind !== "youtubeRadio"
    || (track.provenance as {manualFields?:{artist?:boolean}} | null)?.manualFields?.artist === true
    || track.artist.trim() || track.playbackRange) return track;
  const id = videoId(track);
  if (!id) return track;
  const saved = library.filter(item => item.id === track.id && videoId(item) === id && !item.playbackRange && item.artist.trim());
  if (saved.length !== 1) return track;
  const manual=(track.provenance as {manualFields?:{album?:boolean;albumArtist?:boolean}} | null)?.manualFields;
  return {...track, artist: saved[0].artist, album: manual?.album ? track.album : track.album || saved[0].album,
    albumArtist: manual?.albumArtist ? track.albumArtist : track.albumArtist || saved[0].albumArtist};
}

/** Read the exact saved ID even when it is outside the loaded library page. */
export function createAutoplayMetadataLookup(read: (id: string) => Promise<Track[]>, now = Date.now) {
  const cache = new Map<string, {expires: number; tracks: Track[]}>();
  const pending = new Map<string, Promise<Track[]>>();
  const eligible = (track: Track) => !track.artist.trim() && !track.playbackRange
    && (track.provenance as {manualFields?:{artist?:boolean}} | null)?.manualFields?.artist !== true
    && (track.provenance as Record<string, unknown> | null)?.sourceKind === "youtubeRadio" && videoId(track) !== null;
  const peek = (track: Track): Track => {
    const cached = cache.get(track.id);
    return eligible(track) && cached && cached.expires > now() ? autoplayTrackWithLibraryMetadata(track, cached.tracks) : track;
  };
  return {
    peek,
    async load(track: Track): Promise<Track> {
      if (!eligible(track)) return track;
      const cached = cache.get(track.id);
      if (cached && cached.expires > now()) return autoplayTrackWithLibraryMetadata(track, cached.tracks);
      let request = pending.get(track.id);
      if (!request) {
        request = read(track.id).catch(() => []).then(tracks => {
          const found = autoplayTrackWithLibraryMetadata(track, tracks) !== track;
          if (cache.size >= 64) cache.delete(cache.keys().next().value!);
          cache.set(track.id, {expires: now() + (found ? 10 * 60_000 : 15_000), tracks});
          return tracks;
        }).finally(() => pending.delete(track.id));
        pending.set(track.id, request);
      }
      return autoplayTrackWithLibraryMetadata(track, await request);
    },
  };
}
