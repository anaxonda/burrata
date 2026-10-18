<script lang="ts">
  import { untrack } from "svelte";
  import PlayingInfo from "./PlayingInfo.svelte";
  import type { Track } from "./lib/types";

  let { initialTrack, onArtworkError }: {
    initialTrack: Track;
    onArtworkError: (track: Track, failedUrl: string) => void;
  } = $props();
  let track = $state<Track>(untrack(() => initialTrack));
  let artworkDisplayUrl = $state("");

  export function setArtworkDisplayUrl(value: string) {
    artworkDisplayUrl = value;
  }

  export function setTrack(value: Track) {
    track = value;
  }

  export function currentTrack() {
    return track;
  }
</script>

<PlayingInfo
  {track}
  {artworkDisplayUrl}
  {onArtworkError}
  onClose={() => {}}
  onArtist={() => {}}
  onDownload={() => {}}
  onPlaylist={() => {}}
/>
