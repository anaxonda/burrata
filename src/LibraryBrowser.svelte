<script lang="ts">
  import { tick, onMount, untrack } from 'svelte';
  import ArtworkImage from './ArtworkImage.svelte';
  import TrackList from './TrackList.svelte';
  import LocalAlbumMatch from './LocalAlbumMatch.svelte';
  import { parseDurationBound } from './lib/track-sort';
  import { initialLibraryBrowserState, type LibraryBrowserState, type LibraryEntity, type LibraryEntityPage } from './lib/library';
  import type { Track } from './lib/types';
  let { browserState, call, onNavigate, onStateChange, onBrowse, onPlay, onQueue, onDownload, onFavorite, onPlaylist, onArtist, onAlbum, onContext, onAddSelectedToPlaylist, playingTrack, bufferingTrack, paused = false }: {
    browserState: LibraryBrowserState;
    call: <T = unknown>(command: string, args?: Record<string, unknown>) => Promise<T>;
    onNavigate: (next: LibraryBrowserState) => void;
    onStateChange: (next: LibraryBrowserState) => void;
    onBrowse: (entry: LibraryEntity) => void;
    onPlay: (track: Track, tracks: Track[], index: number) => void | Promise<void>;
    onQueue: (tracks: Track[], mode: 'append' | 'next') => void;
    onDownload: (track: Track) => void; onFavorite: (track: Track) => void; onPlaylist: (track: Track) => void;
    onArtist: (track: Track) => void; onAlbum: (track: Track) => void;
    onContext: (track: Track, event: MouseEvent) => void;
    onAddSelectedToPlaylist: (tracks: Track[]) => void;
    playingTrack?: Track | null; bufferingTrack?: Track | null; paused?: boolean;
  } = $props();
  let items = $state<LibraryEntity[]>([]), tracks = $state<Track[]>([]), total = $state(0), loading = $state(false), error = $state(''), notice = $state('');
  let scrollElement: HTMLElement;
  let generation = 0;
  let showLocalMatch=$state(false);
  const localAlbumIds=$derived(browserState.entity?.kind==='album'?[...new Set([browserState.entity.id,...(browserState.entity.aliases??[])].filter(id=>id.startsWith('album:ref:local:')))]:[]);
  const trackView = $derived(Boolean(browserState.entity && (browserState.entity.kind === 'album' || browserState.section === 'tracks')));
  function change(patch: Partial<LibraryBrowserState>) {
    const next = { ...browserState, ...patch, scroll: 0, loadedCount: 200 };
    if (patch.section && patch.section !== browserState.section) onNavigate(next);
    else onStateChange(next);
  }
  function open(entry: LibraryEntity) { onNavigate({ ...initialLibraryBrowserState(), tab: browserState.tab, entity: entry, section: 'albums' }); }
  async function load(snapshot: LibraryBrowserState, append = false) {
    const request = ++generation;
    loading = true; error = '';
    if (!append) { items = []; tracks = []; total = 0; }
    try {
      const offset = append ? (trackView ? tracks.length : items.length) : 0;
      const limit = 200;
      const wanted = append ? offset + limit : Math.max(limit, snapshot.loadedCount);
      const nextItems: LibraryEntity[] = append ? [...items] : [];
      const nextTracks: Track[] = append ? [...tracks] : [];
      let nextTotal = 0;
      for (let start = offset; start < wanted; start += limit) {
        if (snapshot.entity && (snapshot.entity.kind === 'album' || snapshot.section === 'tracks')) {
          const page = await call<{ tracks: Track[]; total: number }>('list_library_entity_tracks', { id: snapshot.entity.id, query: snapshot.query, sort: snapshot.sort, descending: snapshot.descending, availability: snapshot.availability, minDuration: parseDurationBound(snapshot.minDuration), maxDuration: parseDurationBound(snapshot.maxDuration), limit, offset: start });
          nextTracks.push(...page.tracks); nextTotal = page.total;
        } else {
          const page = await call<LibraryEntityPage>('list_library_entities', { kind: snapshot.entity ? 'album' : snapshot.tab === 'artists' ? 'artist' : 'album', artistId: snapshot.entity?.id ?? null, query: snapshot.query, limit, offset: start });
          nextItems.push(...page.items); nextTotal = page.total;
        }
        if (request !== generation) return;
        if (start + limit >= nextTotal) break;
      }
      if (request !== generation) return;
      items = nextItems; tracks = nextTracks; total = nextTotal;
      await tick();
      if (!append && request === generation && scrollElement) scrollElement.scrollTop = snapshot.scroll;
    } catch (reason) { if (request === generation) error = reason instanceof Error ? reason.message : String(reason); }
    finally { if (request === generation) loading = false; }
  }
  // Scroll updates do not trigger a new query.
  const queryKey = $derived(JSON.stringify([browserState.tab,browserState.entity?.id,browserState.section,browserState.query,browserState.sort,browserState.descending,browserState.availability,browserState.minDuration,browserState.maxDuration]));
  $effect(() => { queryKey; untrack(() => void load({ ...browserState })); return () => { generation++; }; });
  onMount(() => {
    const refresh = () => void load({ ...browserState });
    window.addEventListener('library-changed', refresh);
    return () => { generation++; window.removeEventListener('library-changed', refresh); };
  });
  async function removeSave() {
    const entry = browserState.entity;
    if (!entry) return;
    try {
      await call('remove_library_entity_save', { id: entry.id });
      notice = 'Save removed. Saved tracks are kept, so this entry remains while it contains saved music.';
      onNavigate({ ...initialLibraryBrowserState(), tab: browserState.tab });
      window.dispatchEvent(new Event('library-changed'));
    } catch (reason) { error = String(reason); }
  }
</script>

<section class="library-browser" aria-label="Saved library" bind:this={scrollElement} onscroll={() => onStateChange({ ...browserState, scroll: scrollElement.scrollTop })}>
  {#if browserState.entity}
    <header class="entity-header"><div class="entity-art"><ArtworkImage urls={browserState.entity.artworkUrl ? [browserState.entity.artworkUrl] : []} /></div><div><p>{browserState.entity.kind === 'artist' ? 'Library artist' : 'Library album'}</p><h2>{browserState.entity.name}</h2>{#if browserState.entity.kind === 'album'}<p>{browserState.entity.artist} {browserState.entity.releaseDate?.slice(0,4) ?? ''}</p>{/if}<button onclick={() => onBrowse(browserState.entity!)}>Browse catalog</button>{#if localAlbumIds.length}<button onclick={()=>showLocalMatch=true}>Match local files</button>{/if}{#if browserState.entity.explicitlySaved}<button onclick={() => void removeSave()}>Remove {browserState.entity.kind} save</button>{/if}</div></header>
    {#if browserState.entity.kind === 'artist'}<nav aria-label="Library artist sections"><button aria-pressed={browserState.section === 'albums'} onclick={() => change({ section: 'albums' })}>Albums</button><button aria-pressed={browserState.section === 'tracks'} onclick={() => change({ section: 'tracks' })}>All tracks</button></nav>{/if}
  {/if}
  <div class="controls"><input type="search" aria-label="Filter saved library" placeholder="Filter saved music…" value={browserState.query} oninput={event => change({ query: event.currentTarget.value })} />
    {#if trackView}<select aria-label="Sort saved tracks" value={browserState.sort} onchange={event => change({ sort: event.currentTarget.value })}><option value="sourceOrder">Disc / track order</option><option value="title">Title</option><option value="artist">Artist</option><option value="album">Album</option><option value="releaseDate">Release date</option><option value="duration">Duration</option></select><button aria-label="Toggle saved track sort direction" onclick={() => change({ descending: !browserState.descending })}>{browserState.descending ? 'Descending' : 'Ascending'}</button><select aria-label="Saved track availability" value={browserState.availability} onchange={event => change({ availability: event.currentTarget.value })}><option value="all">All recordings</option><option value="local">On this device</option><option value="not-local">Not on this device</option></select><input aria-label="Minimum saved track duration" placeholder="From mm:ss" value={browserState.minDuration} oninput={event => change({ minDuration: event.currentTarget.value })} /><input aria-label="Maximum saved track duration" placeholder="To mm:ss" value={browserState.maxDuration} oninput={event => change({ maxDuration: event.currentTarget.value })} />{/if}
  </div>
  {#if notice}<p role="status">{notice}</p>{/if}
  {#if error}<p role="alert">{error}</p><button onclick={() => void load({ ...browserState })}>Retry</button>{/if}
  {#if loading}<p role="status">Loading saved music…</p>{/if}
  {#if !loading && !error && !total}<p>No saved music here yet.{#if browserState.entity} Browse the catalog to find music to save.{/if}</p>{/if}
  {#if trackView}
    <div class="controls"><button disabled={!tracks.length} onclick={() => void onPlay(tracks[0], tracks, 0)}>Play saved tracks</button><button disabled={!tracks.length} onclick={() => onQueue(tracks, 'append')}>Queue saved tracks</button><small>{tracks.length} of {total} saved tracks loaded</small></div>
    <TrackList {tracks} {onPlay} {onDownload} {onFavorite} {onPlaylist} {onArtist} {onAlbum} {onContext} {onAddSelectedToPlaylist} onQueue={track => onQueue([track], 'append')} onNext={track => onQueue([track], 'next')} {playingTrack} {bufferingTrack} {paused} listLabel="Saved tracks" selectionContext={`library:${browserState.entity?.id}`} />
  {:else}<div class="entity-grid">{#each items as entry (entry.id)}<button class="entity-card" onclick={() => open(entry)}><div class="card-art"><ArtworkImage urls={entry.artworkUrl ? [entry.artworkUrl] : []} /></div><strong>{entry.name}</strong>{#if entry.kind === 'album'}<span>{entry.artist}</span><small>{entry.releaseDate?.slice(0,4) ?? ''} · {entry.trackCount}{entry.totalTracks != null ? ` of ${entry.totalTracks}` : ''} tracks saved</small>{:else}<small>{entry.trackCount} saved tracks · {entry.albumCount} albums</small>{/if}{#if entry.explicitlySaved}<small>Saved {entry.kind}</small>{/if}{#if entry.provider}<small>{entry.provider}</small>{/if}</button>{/each}</div>{/if}
  {#if (trackView ? tracks.length : items.length) < total}<button disabled={loading} onclick={() => { onStateChange({ ...browserState, loadedCount: (trackView ? tracks.length : items.length) + 200 }); void load({ ...browserState }, true); }}>Load more</button>{/if}
</section>
{#if showLocalMatch && localAlbumIds.length}<LocalAlbumMatch {localAlbumIds} {call} onClose={()=>showLocalMatch=false} onChanged={entry=>{showLocalMatch=false;onNavigate({...browserState,entity:entry,scroll:0,loadedCount:200});}} />{/if}

<style>
  .library-browser{min-height:0;flex:1;overflow:auto;padding:1rem;display:flex;flex-direction:column;gap:1rem}.entity-header{display:flex;align-items:center;gap:1rem}.entity-header h2{margin:.3rem 0}.entity-header p{margin:.3rem 0;color:var(--muted)}.entity-art{width:100px;height:100px;flex:none;overflow:hidden;border-radius:8px}.controls,nav{display:flex;flex-wrap:wrap;align-items:center;gap:.5rem}.controls input{min-width:160px}.entity-grid{display:grid;grid-template-columns:repeat(auto-fill,minmax(155px,1fr));gap:1rem}.entity-card{display:flex;flex-direction:column;gap:.4rem;padding:.7rem;text-align:left;min-width:0}.card-art{width:100%;aspect-ratio:1;overflow:hidden;border-radius:6px}.entity-card span,.entity-card small{color:var(--muted);overflow-wrap:anywhere}button[aria-pressed="true"]{border-color:var(--accent);color:var(--accent)}
</style>
