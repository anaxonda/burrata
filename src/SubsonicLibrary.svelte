<script lang="ts">
  import TrackList from "./TrackList.svelte";
  import AlbumDetailView from "./AlbumDetailView.svelte";
  import type { Track } from "./lib/types";
  import { subsonicArtworkDataUrl, type SubsonicPage, type SubsonicScope, type SubsonicStatus } from "./lib/subsonic";

  let { call, provider = "subsonic", status, scope, onStatus, onClearScope, onArtist, onAlbum, onPlay, onFavorite, onPlaylist, onAddSelectedToPlaylist, onAddAlbumToPlaylist, onQueue, favoriteIds,
    playingTrack, bufferingTrack, paused, playbackFailed }: {
    call: <T>(command: string, args?: Record<string, unknown>) => Promise<T>;
    provider?: "subsonic" | "jellyfin";
    status: SubsonicStatus;
    scope?: SubsonicScope;
    onClearScope?: () => void;
    onArtist?: (track: Track) => void;
    onAlbum?: (track: Track) => void;
    onStatus: (status: SubsonicStatus) => void;
    onPlay: (track: Track, tracks: Track[], index: number) => void | Promise<void>;
    onFavorite: (track: Track) => void;
    onPlaylist: (track: Track) => void;
    onAddSelectedToPlaylist?: (tracks: Track[]) => void;
    onAddAlbumToPlaylist?: (tracks: Track[], title: string) => void | Promise<void>;
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
  let snapshotId = $state<string | null>(null);
  let error = $state("");
  let busy = $state(false);
  let refreshing = $state(false);
  let catalogRevision = $state(0);
  let albumArtwork = $state<string | null>(null);

  const pageSize = 100;
  const sourceLabel = $derived(provider === "jellyfin" ? "Jellyfin" : "Subsonic");
  const albumView = $derived(scope?.kind === "album");
  const favorites = $derived(new Set(favoriteIds));
  const visibleTracks = $derived(tracks.map(track => ({ ...track, favorite: favorites.has(track.id) })));
  const albumArtist = $derived(tracks[0]?.albumArtist?.trim() || tracks[0]?.artist || "");
  const creditedAlbumArtist = $derived(tracks.find(track => track.artist.trim() === albumArtist));
  const coverSource = $derived(tracks[0]?.artworkUrl);

  $effect(() => {
    const query = filter.trim();
    const start = offset;
    const target = scope && { kind: scope.kind, trackId: scope.trackId };
    catalogRevision;
    status.trackCount;
    status.endpoint;
    status.username;
    let active = true;
    const load = async () => {
      busy = true;
      error = "";
      try {
        const next = await call<SubsonicPage>(`browse_${provider}`, { query, offset: start, limit: pageSize, ...(target && { scope: target }) });
        if (!active) return;
        const rows = next?.tracks ?? [];
        if (target?.kind === "album" && start > 0) {
          const known = new Set(tracks.map(row => row.id));
          if (tracks.length !== start || next.total !== total || (next.snapshotId ?? null) !== snapshotId ||
              rows.length !== Math.min(pageSize, total - start) || rows.some(row => known.has(row.id))) {
            offset = 0;
            throw new Error("The album changed while loading. Reloading its track list.");
          }
          tracks = [...tracks, ...rows];
        } else {
          tracks = rows;
        }
        total = next?.total ?? 0;
        snapshotId = next?.snapshotId ?? null;
      } catch (failure) {
        if (active) error = String(failure);
      } finally {
        if (active) busy = false;
      }
    };
    const timer = setTimeout(() => void load(), query ? 150 : 0);
    return () => { active = false; clearTimeout(timer); };
  });

  $effect(() => {
    const target = scope;
    if (target?.kind !== "album") return;
    albumArtwork = coverSource ?? null;
    if (coverSource) return;
    let active = true;
    void call<unknown>(`get_${provider}_track_artwork`, { trackId: target.trackId })
      .then(value => { if (active) albumArtwork = subsonicArtworkDataUrl(value); })
      .catch(() => {});
    return () => { active = false; };
  });

  async function resolveAlbumTracks(): Promise<Track[]> {
    if (scope?.kind !== "album" || !total) return [];
    const owner = scope.trackId;
    const revision = catalogRevision;
    const snapshot = snapshotId;
    const expectedTotal = total;
    const result = [...visibleTracks];
    const known = new Set(result.map(track => track.id));
    for (let start = result.length; start < expectedTotal; start = result.length) {
      const page = await call<SubsonicPage>(`browse_${provider}`, {
        scope: { kind: "album", trackId: owner }, query: "", offset: start, limit: pageSize,
      });
      if (scope?.trackId !== owner || catalogRevision !== revision || snapshotId !== snapshot || total !== expectedTotal ||
          (page.snapshotId ?? null) !== snapshot || page.total !== expectedTotal ||
          page.tracks.length !== Math.min(pageSize, expectedTotal - start)) {
        throw new Error(`The ${sourceLabel} album changed while loading. Try again.`);
      }
      for (const track of page.tracks) {
        if (known.has(track.id)) throw new Error("The Subsonic album changed while loading. Try again.");
        known.add(track.id);
        result.push(track);
      }
    }
    return result;
  }

  async function refresh() {
    refreshing = true;
    error = "";
    try {
      onStatus(await call<SubsonicStatus>(`refresh_${provider}`));
      offset = 0;
      catalogRevision++;
    } catch (failure) {
      error = String(failure);
    } finally {
      refreshing = false;
    }
  }
</script>

<section class="subsonic-library" aria-label={`${sourceLabel} library`}>
  {#if scope}
    <div class="catalog-scope">
      <button type="button" onclick={onClearScope}>All {sourceLabel} tracks</button>
      {#if !albumView}<h2>{scope.label}</h2>{/if}
    </div>
  {/if}
  {#if albumView && scope}
    <AlbumDetailView ownsScroll={false} metadata={{ title: scope.label, artist: albumArtist, artworkUrl: albumArtwork, sourceLabel, year: tracks[0]?.releaseDate, trackCountLabel: total ? `${total} tracks` : null }}
      albumTracks={visibleTracks} {resolveAlbumTracks} actionTrackCount={total} loading={busy && !tracks.length} analysisIdentity={scope.trackId}
      onPlayAlbum={items => { if (items.length) return onPlay(items[0], items, 0); }} onQueueAlbum={items => onQueue(items, "append")}
      {onAddAlbumToPlaylist} onArtistClick={creditedAlbumArtist && onArtist ? () => onArtist?.(creditedAlbumArtist!) : undefined}
      trackListProps={{ tracks: visibleTracks, listLabel: `${scope.label} tracks`, onPlay, onFavorite, onPlaylist, onArtist, onAddSelectedToPlaylist,
        onQueue: track => onQueue([track], "append"), onNext: track => onQueue([track], "next"),
        playingTrack, bufferingTrack, paused, playbackFailed }}>
      {#snippet notices()}{#if error}<p role="alert">{error}</p>{/if}{/snippet}
    </AlbumDetailView>
    {#if tracks.length < total}<button type="button" disabled={busy} onclick={() => { offset = tracks.length; }}>Load more album tracks</button>{/if}
  {:else}
  <div class="catalog-tools">
    <input type="search" aria-label={`Filter ${sourceLabel} tracks`} placeholder="Filter by track, album, or artist"
      bind:value={filter} oninput={() => offset = 0} />
    <button type="button" disabled={refreshing} onclick={() => void refresh()}>{refreshing ? "Refreshing catalog…" : "Refresh catalog"}</button>
  </div>
  {#if error}<p role="alert">{error}</p>{/if}
  {#if visibleTracks.length}
    <TrackList tracks={visibleTracks} presentation="playlist" listLabel={`${sourceLabel} tracks`} position={(_, index) => offset + index + 1}
      {onPlay} {onFavorite} {onPlaylist} {onAddSelectedToPlaylist} {onArtist} {onAlbum} onQueue={track => onQueue([track], "append")} onNext={track => onQueue([track], "next")}
      {playingTrack} {bufferingTrack} {paused} {playbackFailed} />
    <nav class="catalog-pagination" aria-label={`${sourceLabel} catalog pages`}>
      <button type="button" disabled={busy || offset === 0} onclick={() => offset = Math.max(0, offset - pageSize)}>Previous</button>
      <span>{offset + 1}–{offset + visibleTracks.length} of {total.toLocaleString()}</span>
      <button type="button" disabled={busy || offset + pageSize >= total} onclick={() => offset += pageSize}>Next</button>
    </nav>
  {:else if busy}
    <p role="status">Loading {sourceLabel} tracks…</p>
  {:else if !error}
    <p>{filter.trim() || scope ? `No ${sourceLabel} tracks match this view.` : `Refresh the catalog to load your ${sourceLabel} tracks.`}</p>
  {/if}
  {/if}
</section>

<style>
  .subsonic-library { flex: 1; min-height: 0; min-width: 0; overflow: auto; padding: 1rem; }
  .catalog-scope { display: flex; flex-wrap: wrap; align-items: center; gap: .75rem; margin-block-end: .75rem; }
  .catalog-scope h2 { margin: 0; }
  .catalog-tools { display: flex; flex-wrap: wrap; gap: .5rem; margin-block-end: .5rem; }
  .catalog-tools input { flex: 1; min-width: 10rem; }
  .catalog-pagination { display: flex; justify-content: center; align-items: center; gap: .65rem; margin-block-start: .75rem; }
</style>
