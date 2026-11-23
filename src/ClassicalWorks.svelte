<script lang="ts">
  import { onMount, type ComponentProps } from "svelte";
  import TrackList from "./TrackList.svelte";
  import { classicalTrackListRows, classicalWorkTracksInAlbumOrder, filterClassicalWorks, findClassicalWorks, type WorkProgramme } from "./lib/classical-works";
  import type { Track } from "./lib/types";

  type TrackActions = Pick<ComponentProps<typeof TrackList>,
    "onPlay" | "onQueue" | "onNext" | "onDownload" | "onFavorite" | "onArtist" | "onAlbum" | "onPlaylist" | "onContext"
    | "playingTrack" | "bufferingTrack" | "bufferingTrackId" | "playbackFailed" | "paused" | "showProviderCounts">;
  let { call, query, workKey, onQuery, onOpen, trackActions }: {
    call: <T = unknown>(command: string, args?: Record<string, unknown>) => Promise<T>;
    query: string;
    workKey: string | null;
    onQuery: (value: string) => void;
    onOpen: (key: string | null) => void;
    trackActions: TrackActions;
  } = $props();

  const pageSize = 200;
  const maxTracks = 10_000;
  let tracks = $state<Track[]>([]);
  let loading = $state(true);
  let truncated = $state(false);
  let error = $state("");
  let playbackError = $state("");
  let generation = 0;
  const workIndex = $derived(findClassicalWorks(tracks));
  const matches = $derived(filterClassicalWorks(workIndex, query));
  const work = $derived(workKey ? workIndex.find(entry => entry.key === workKey) : undefined);
  const workTracks = $derived(work ? classicalWorkTracksInAlbumOrder(work.tracks) : []);
  const orderedRows = $derived(classicalTrackListRows(workTracks));
  $effect(() => { workKey; playbackError = ""; });

  async function load() {
    const request = ++generation;
    loading = true;
    error = "";
    try {
      const result: Track[] = [];
      const seen = new Set<string>();
      let total: number | null = null;
      for (let offset = 0; offset < maxTracks && (total === null || offset < total); offset += pageSize) {
        const page = await call<{ tracks: Track[]; total: number }>("list_tracks_page", { view: "works", limit: pageSize, offset });
        if (request !== generation) return;
        if (!Array.isArray(page.tracks) || !Number.isSafeInteger(page.total) || page.total < 0
            || (total !== null && page.total !== total)
            || page.tracks.length !== Math.min(pageSize, Math.max(0, page.total - offset))) {
          throw new Error("The library changed while loading works. Try again.");
        }
        for (const track of page.tracks) {
          if (seen.has(track.id)) throw new Error("The library changed while loading works. Try again.");
          seen.add(track.id);
          result.push(track);
        }
        total = page.total;
      }
      tracks = result;
      truncated = (total ?? 0) > maxTracks;
    } catch (failure) {
      if (request === generation) error = failure instanceof Error ? failure.message : String(failure);
    } finally {
      if (request === generation) loading = false;
    }
  }
  onMount(() => {
    const refresh = () => void load();
    refresh();
    window.addEventListener("library-changed", refresh);
    return () => { generation++; window.removeEventListener("library-changed", refresh); };
  });

  async function play(items: Track[]) {
    const key = workKey;
    playbackError = "";
    try {
      if (items[0]) await trackActions.onPlay?.(items[0], items, 0);
    } catch (failure) {
      if (key === workKey) playbackError = failure instanceof Error ? failure.message : String(failure);
    }
  }
  function playProgramme(programme: WorkProgramme) { return play(programme.tracks); }
</script>

<section class="classical-works" aria-label="Classical works">
  {#if loading && !tracks.length}
    <p role="status">Loading saved works…</p>
  {:else if error}
    <p role="alert">{error}</p>
    <button type="button" onclick={() => void load()}>Retry</button>
  {:else if workKey}
    <button type="button" class="works-back" onclick={() => onOpen(null)}>← Back to works</button>
    {#if work}
      <h2>{work.title}</h2>
      {#if work.composers.length}<p class="work-composers">{work.composers.join(" · ")}</p>{/if}
      {#if playbackError}<p role="alert">{playbackError}</p>{/if}
      <TrackList tracks={workTracks} {orderedRows} {...trackActions} listLabel={`Recordings of ${work.title}`}
        onPlay={track => play([track])} onPlayWork={playProgramme} presentation="playlist" />
    {:else}
      <p>This work is no longer in the saved library.</p>
    {/if}
  {:else}
    <h2>Works</h2>
    <input type="search" aria-label="Search saved works" placeholder="Search by work, composer, or catalog number"
      value={query} oninput={event => onQuery(event.currentTarget.value)} />
    {#if truncated}<p role="status">Showing works from the first {maxTracks.toLocaleString()} tagged or catalog recordings.</p>{/if}
    {#if matches.length}
      <ul class="work-results">
        {#each matches as entry (entry.key)}
          <li><button type="button" onclick={() => onOpen(entry.key)}>
            <strong>{entry.title}</strong>
            <span>{entry.composers.join(" · ")}{entry.composers.length ? " · " : ""}{entry.tracks.length} saved {entry.tracks.length === 1 ? "recording" : "recordings"}</span>
          </button></li>
        {/each}
      </ul>
    {:else}
      <p>{query.trim() ? "No saved works match that search." : "No classical works are saved yet. Save a MusicBrainz recording or import files with work and movement tags."}</p>
    {/if}
  {/if}
</section>

<style>
  .classical-works{display:grid;align-content:start;gap:.65rem;min-width:0}
  .classical-works h2,.classical-works p{margin:0}
  .classical-works input{width:min(100%,28rem);box-sizing:border-box}
  .works-back{justify-self:start}
  .work-composers,.work-results span{color:var(--muted)}
  .work-results{display:grid;gap:.3rem;list-style:none;margin:0;padding:0}
  .work-results button{display:grid;gap:.2rem;width:100%;text-align:start;overflow-wrap:anywhere}
</style>
