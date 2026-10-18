<script lang="ts">
  import ReleaseView from "./ReleaseView.svelte";
  import type { ArtistReleaseRef } from "./lib/artist-types";
  import type { Track } from "./lib/types";
  let { release, tracks, onSaveToLibrary }: { release: ArtistReleaseRef; tracks: Track[]; onSaveToLibrary: (tracks: Track[]) => Promise<void> } = $props();
  let activeRelease = $state<ArtistReleaseRef>();
  $effect(() => { if (!activeRelease) activeRelease = release; });
  export function setRelease(next: ArtistReleaseRef) { activeRelease = next; }
</script>
{#if activeRelease}<ReleaseView release={activeRelease} {tracks} {onSaveToLibrary} />{/if}
