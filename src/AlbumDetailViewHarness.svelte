<script lang="ts">
  import AlbumDetailView from "./AlbumDetailView.svelte";
  import type { Track } from "./lib/types";
  let identity = $state("album:first");
  let albumTracks = $state<Track[]>([]);
  let resolveAlbumTracks = $state<() => Promise<Track[]>>(async () => []);
  let onPlayAlbum = $state<(tracks: Track[]) => void>(() => {});
  export function updateAlbum(next: { identity: string; tracks: Track[]; resolve: () => Promise<Track[]>; onPlay: (tracks: Track[]) => void }) {
    identity = next.identity; albumTracks = next.tracks; resolveAlbumTracks = next.resolve; onPlayAlbum = next.onPlay;
  }
</script>

<AlbumDetailView
  metadata={{ title: identity, artist: "Example" }} {albumTracks} {resolveAlbumTracks}
  analysisIdentity={identity} actionTrackCount={2} trackListProps={{ tracks: albumTracks }}
  onPlayAlbum={(tracks) => onPlayAlbum(tracks)} onQueueAlbum={() => {}}
/>
