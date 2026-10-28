<script lang="ts">
  import { untrack } from "svelte";
  import LyricsPanel, { type CurrentTrackLyrics } from "./LyricsPanel.svelte";
  import type { Track } from "./lib/types";
  let { initialTrack, loadLyrics, initialPosition = 0 }: { initialTrack: Track; loadLyrics: () => Promise<CurrentTrackLyrics>; initialPosition?:number } = $props();
  let track = $state(untrack(()=>initialTrack));
  let position = $state(untrack(()=>initialPosition));
  export function replaceTrack(next: Track) { track = next; }
  export function replacePosition(next:number) { position=next; }
</script>
<LyricsPanel {track} absolutePosition={position} {loadLyrics} />
