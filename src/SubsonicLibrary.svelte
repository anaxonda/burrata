<script lang="ts">
  import TrackList from "./TrackList.svelte";
  import type { Track } from "./lib/types";
  import type { SubsonicPage, SubsonicStatus } from "./lib/subsonic";

  let { call, status, onStatus, onPlay, onFavorite, onPlaylist, onQueue, favoriteIds,
    playingTrack, bufferingTrack, paused, playbackFailed }: {
    call: <T>(command: string, args?: Record<string, unknown>) => Promise<T>;
    status: SubsonicStatus;
    onStatus: (status: SubsonicStatus) => void;
    onPlay: (track: Track, tracks: Track[], index: number) => void | Promise<void>;
    onFavorite: (track: Track) => void;
    onPlaylist: (track: Track) => void;
    onQueue: (tracks: Track[], placement: "append" | "next") => void;
    favoriteIds: readonly string[];
    playingTrack?: Track | null;
    bufferingTrack?: Track | null;
    paused?: boolean;
    playbackFailed?: boolean;
  } = $props();

  let filter = $state("");
  let offset = $state(0);
  let tracks = $state<Track[]>([]);
  let total = $state(0);
  let error = $state("");
  let busy = $state(false);
  let refreshing = $state(false);
  let catalogRevision = $state(0);

  const pageSize = 100;
  const favorites = $derived(new Set(favoriteIds));
  const visibleTracks = $derived(tracks.map(track => ({ ...track, favorite: favorites.has(track.id) })));

  $effect(() => {
    const query = filter.trim();
    const start = offset;
    catalogRevision;
    status.trackCount;
    let active = true;
    const load = async () => {
      busy = true;
      error = "";
      try {
        const next = await call<SubsonicPage>("browse_subsonic", { query, offset: start, limit: pageSize });
        if (!active) return;
        tracks = next?.tracks ?? [];
        total = next?.total ?? 0;
      } catch (failure) {
        if (active) error = String(failure);
      } finally {
        if (active) busy = false;
      }
    };
    const timer = setTimeout(() => void load(), query ? 150 : 0);
    return () => { active = false; clearTimeout(timer); };
  });

  async function refresh() {
    refreshing = true;
    error = "";
    try {
      onStatus(await call<SubsonicStatus>("refresh_subsonic"));
      offset = 0;
      catalogRevision++;
    } catch (failure) {
      error = String(failure);
    } finally {
      refreshing = false;
    }
  }
</script>

<section class="subsonic-library" aria-label="Subsonic library">
  <div class="catalog-tools">
    <input type="search" aria-label="Filter Subsonic tracks" placeholder="Filter by track, album, or artist"
      bind:value={filter} oninput={() => offset = 0} />
    <button type="button" disabled={refreshing} onclick={() => void refresh()}>{refreshing ? "Refreshing catalog…" : "Refresh catalog"}</button>
  </div>
  {#if error}<p role="alert">{error}</p>{/if}
  {#if visibleTracks.length}
    <TrackList tracks={visibleTracks} presentation="playlist" listLabel="Subsonic tracks" position={(_, index) => offset + index + 1}
      {onPlay} {onFavorite} {onPlaylist} onQueue={track => onQueue([track], "append")} onNext={track => onQueue([track], "next")}
      {playingTrack} {bufferingTrack} {paused} {playbackFailed} />
    <nav class="catalog-pagination" aria-label="Subsonic catalog pages">
      <button type="button" disabled={busy || offset === 0} onclick={() => offset = Math.max(0, offset - pageSize)}>Previous</button>
      <span>{offset + 1}–{offset + visibleTracks.length} of {total.toLocaleString()}</span>
      <button type="button" disabled={busy || offset + pageSize >= total} onclick={() => offset += pageSize}>Next</button>
    </nav>
  {:else if busy}
    <p role="status">Loading Subsonic tracks…</p>
  {:else if !error}
    <p>{filter.trim() ? "No Subsonic tracks match this filter." : "Refresh the catalog to load your Subsonic tracks."}</p>
  {/if}
</section>

<style>
  .subsonic-library { flex: 1; min-height: 0; min-width: 0; overflow: auto; padding: 1rem; }
  .catalog-tools { display: flex; flex-wrap: wrap; gap: .5rem; margin-block-end: .5rem; }
  .catalog-tools input { flex: 1; min-width: 10rem; }
  .catalog-pagination { display: flex; justify-content: center; align-items: center; gap: .65rem; margin-block-start: .75rem; }
</style>
