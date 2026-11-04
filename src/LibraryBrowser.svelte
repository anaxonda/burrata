<script lang="ts">
  import { tick, onMount, untrack } from 'svelte';
  import ArtworkImage from './ArtworkImage.svelte';
  import TrackList from './TrackList.svelte';
  import AlbumDetailView from './AlbumDetailView.svelte';
  import LocalAlbumMatch from './LocalAlbumMatch.svelte';
  import { parseDurationBound } from './lib/track-sort';
  import { groupLibraryEntities, initialLibraryBrowserState, saveLibraryEntityLayouts, type LibraryBrowserState, type LibraryEntity, type LibraryEntityPage } from './lib/library';
  import type { Track } from './lib/types';
  import type { LoudnessResourceIdentity } from './lib/loudness-normalization';
  let { browserState, call, onNavigate, onStateChange, onBrowse, onPlay, onQueue, onDownload, onFavorite, onPlaylist, onArtist, onAlbum, onContext, onAddSelectedToPlaylist, onDownloadAlbum, onSearchAlbumSoulseek, soulseekSearchAvailable = false, onAddAlbumToPlaylist, onAnalyzeLoudness, playingTrack, bufferingTrack, bufferingTrackId = '', playbackFailed = false, paused = false }: {
    browserState: LibraryBrowserState;
    call: <T = unknown>(command: string, args?: Record<string, unknown>) => Promise<T>;
    onNavigate: (next: LibraryBrowserState) => void;
    onStateChange: (next: LibraryBrowserState) => void;
    onBrowse: (entry: LibraryEntity) => void;
    onPlay: (track: Track, tracks: Track[], index: number, resource?: LoudnessResourceIdentity | null) => void | Promise<void>;
    onQueue: (tracks: Track[], mode: 'append' | 'next', resource?: LoudnessResourceIdentity | null) => void;
    onDownload: (track: Track) => void; onFavorite: (track: Track) => void; onPlaylist: (track: Track) => void;
    onArtist: (track: Track) => void; onAlbum: (track: Track) => void;
    onContext: (track: Track, event: MouseEvent) => void;
    onAddSelectedToPlaylist: (tracks: Track[]) => void;
    onDownloadAlbum?: (tracks: Track[]) => void | Promise<void>;
    onSearchAlbumSoulseek?: (entry: LibraryEntity) => void | Promise<void>;
    soulseekSearchAvailable?: boolean;
    onAddAlbumToPlaylist?: (tracks: Track[], title: string) => void | Promise<void>;
    onAnalyzeLoudness?: (tracks: Track[], gapSeconds: number) => Promise<{ jobId: string; playbackTracks: Track[] }>;
    playingTrack?: Track | null; bufferingTrack?: Track | null; bufferingTrackId?: string; playbackFailed?: boolean; paused?: boolean;
  } = $props();
  let items = $state<LibraryEntity[]>([]), tracks = $state<Track[]>([]), total = $state(0), loading = $state(false), error = $state(''), notice = $state('');
  let trackActionPending = $state(''), trackActionError = $state('');
  let scopeRevision = $state(0);
  let scrollElement: HTMLElement;
  let generation = 0;
  let trackActionGeneration = 0;
  const actionTrackLimit = 10000;
  let showLocalMatch=$state(false);
  const entityGroups = $derived(groupLibraryEntities(items));
  const localAlbumIds=$derived(browserState.entity?.kind==='album'?[...new Set([browserState.entity.id,...(browserState.entity.aliases??[])].filter(id=>id.startsWith('album:ref:local:')))]:[]);
  const trackView = $derived(Boolean(browserState.entity && (browserState.entity.kind === 'album' || browserState.section === 'tracks')));
  const albumDetailView = $derived(browserState.entity?.kind === 'album');
  const artistListView = $derived(browserState.tab === 'artists' && !browserState.entity);
  const entityLayout = $derived(artistListView ? browserState.artistLayout : browserState.albumLayout);
  const layoutLabel = $derived(artistListView ? 'artists' : 'albums');
  function setEntityLayout(layout: 'grid' | 'list') {
    const next = artistListView ? { ...browserState, artistLayout: layout } : { ...browserState, albumLayout: layout };
    saveLibraryEntityLayouts({ artistLayout: next.artistLayout, albumLayout: next.albumLayout });
    onStateChange(next);
  }
  function change(patch: Partial<LibraryBrowserState>) {
    const next = { ...browserState, ...patch, scroll: 0, loadedCount: 200 };
    if (patch.section && patch.section !== browserState.section) onNavigate(next);
    else onStateChange(next);
  }
  function open(entry: LibraryEntity) { onNavigate({ ...initialLibraryBrowserState(), tab: browserState.tab, entity: entry, section: 'albums' }); }
  function trackQueryArgs(snapshot: LibraryBrowserState, limit: number, offset: number) {
    return { id: snapshot.entity!.id, query: snapshot.query, sort: snapshot.sort, descending: snapshot.descending, availability: snapshot.availability, minDuration: parseDurationBound(snapshot.minDuration), maxDuration: parseDurationBound(snapshot.maxDuration), limit, offset };
  }
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
          const page = await call<{ tracks: Track[]; total: number }>('list_library_entity_tracks', trackQueryArgs(snapshot, limit, start));
          nextTracks.push(...page.tracks); nextTotal = page.total;
        } else {
          const page = await call<LibraryEntityPage>('list_library_entities', { kind: snapshot.entity ? 'album' : snapshot.tab === 'artists' ? 'artist' : 'album', artistId: snapshot.entity?.id ?? null, query: snapshot.query, source: snapshot.entitySource || null, year: snapshot.entityYear || null, sort: snapshot.entitySort, descending: snapshot.entityDescending, limit, offset: start });
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
  async function readAllMatchingTracks(snapshot: LibraryBrowserState, isCurrent: () => boolean): Promise<Track[] | null> {
    if (!snapshot.entity || (snapshot.entity.kind !== 'album' && snapshot.section !== 'tracks')) {
      throw new Error('Open a saved album or artist track view before using these actions.');
    }
    const limit = 200;
    const all: Track[] = [];
    let expectedTotal: number | null = null;
    for (let offset = 0; expectedTotal === null || offset < expectedTotal; offset += limit) {
      if (!isCurrent()) return null;
      const page = await call<{ tracks: Track[]; total: number }>('list_library_entity_tracks', trackQueryArgs(snapshot, limit, offset));
      if (!isCurrent()) return null;
      if (expectedTotal === null) {
        expectedTotal = page.total;
        if (expectedTotal > actionTrackLimit) {
          throw new Error(`This action supports up to ${actionTrackLimit.toLocaleString()} matching tracks. Narrow the filters and try again.`);
        }
      } else if (page.total !== expectedTotal) {
        throw new Error('The saved track list changed while loading. Nothing was queued or played; try again.');
      }
      const expectedPageLength = Math.min(limit, Math.max(0, expectedTotal - offset));
      if (page.tracks.length !== expectedPageLength) {
        throw new Error('Could not load every matching saved track. Nothing was queued or played; try again.');
      }
      all.push(...page.tracks);
    }
    return all;
  }
  async function resolveAlbumTracks(): Promise<Track[]> {
    const request = trackActionGeneration;
    const snapshotKey = queryKey;
    const isCurrent = () => request === trackActionGeneration && snapshotKey === queryKey;
    const entry = browserState.entity;
    if (!entry || entry.kind !== 'album') throw new Error('Open a saved album before using album-wide actions.');
    const canonicalSnapshot = { ...browserState, query: '', sort: 'sourceOrder', descending: false, availability: 'all', minDuration: '', maxDuration: '' };
    const all = await readAllMatchingTracks(canonicalSnapshot, isCurrent);
    if (!isCurrent() || !all) throw new Error('The saved album view changed while loading. Try again.');
    if (!all.length) throw new Error('There are no saved tracks in this album scope.');
    return all;
  }
  async function analyzeCompleteSavedAlbum(items: Track[], gapSeconds: number) {
    const entry = browserState.entity;
    if (items.length < 2) throw new Error('At least two saved album tracks are required for analysis.');
    if (!entry || entry.kind !== 'album' || entry.totalTracks == null || items.length !== entry.totalTracks) {
      throw new Error('The complete album tracklist is no longer saved. Refresh the Library after saving the missing tracks, then retry analysis.');
    }
    if (!onAnalyzeLoudness) throw new Error('Album loudness analysis is unavailable.');
    return await onAnalyzeLoudness(items, gapSeconds);
  }
  async function runTrackAction(action: 'play' | 'queue') {
    const request = ++trackActionGeneration;
    const snapshot = { ...browserState };
    const snapshotKey = queryKey;
    const isCurrent = () => request === trackActionGeneration && snapshotKey === queryKey;
    trackActionPending = action;
    trackActionError = '';
    try {
      const all = await readAllMatchingTracks(snapshot, isCurrent);
      if (!isCurrent() || !all?.length) return;
      if (action === 'play') await onPlay(all[0]!, all, 0);
      else onQueue(all, 'append');
    } catch (reason) {
      if (isCurrent()) trackActionError = reason instanceof Error ? reason.message : String(reason);
    } finally {
      if (request === trackActionGeneration) trackActionPending = '';
    }
  }
  // Scroll updates do not trigger a new query.
  const queryKey = $derived(JSON.stringify([browserState.tab,browserState.entity?.id,browserState.section,browserState.query,browserState.sort,browserState.descending,browserState.availability,browserState.minDuration,browserState.maxDuration,browserState.entitySource,browserState.entityYear,browserState.entitySort,browserState.entityDescending]));
  $effect(() => { queryKey; trackActionGeneration++; trackActionPending = ''; trackActionError = ''; untrack(() => void load({ ...browserState })); return () => { generation++; trackActionGeneration++; trackActionPending = ''; }; });
  onMount(() => {
    const refresh = () => { scopeRevision++; trackActionGeneration++; trackActionPending = ''; trackActionError = ''; void load({ ...browserState }); };
    window.addEventListener('library-changed', refresh);
    return () => { generation++; trackActionGeneration++; window.removeEventListener('library-changed', refresh); };
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
  {#if browserState.entity?.kind === 'artist'}
    <header class="entity-header"><div class="entity-art"><ArtworkImage urls={browserState.entity.artworkUrl ? [browserState.entity.artworkUrl] : []} /></div><div><p>Library artist</p><h2>{browserState.entity.name}</h2><button onclick={() => onBrowse(browserState.entity!)}>Browse catalog</button>{#if browserState.entity.explicitlySaved}<button onclick={() => void removeSave()}>Remove artist save</button>{/if}</div></header>
    <nav aria-label="Library artist sections"><button aria-pressed={browserState.section === 'albums'} onclick={() => change({ section: 'albums' })}>Albums</button><button aria-pressed={browserState.section === 'tracks'} onclick={() => change({ section: 'tracks' })}>All tracks</button></nav>
  {/if}
  {#if !albumDetailView}<div class="controls"><input type="search" aria-label={trackView ? 'Filter saved tracks' : browserState.entity ? 'Filter saved albums' : browserState.tab === 'artists' ? 'Filter saved artists' : 'Filter saved albums'} placeholder={trackView ? 'Filter saved tracks…' : browserState.entity ? 'Filter albums by title, artist, year, or source…' : browserState.tab === 'artists' ? 'Filter artists by name or source…' : 'Filter albums by title, artist, year, or source…'} value={browserState.query} oninput={event => change({ query: event.currentTarget.value })} />
    {#if trackView}<select aria-label="Sort saved tracks" value={browserState.sort} onchange={event => change({ sort: event.currentTarget.value })}><option value="sourceOrder">Disc / track order</option><option value="title">Title</option><option value="artist">Artist</option><option value="album">Album</option><option value="releaseDate">Release date</option><option value="duration">Duration</option></select><button aria-label="Toggle saved track sort direction" onclick={() => change({ descending: !browserState.descending })}>{browserState.descending ? 'Descending' : 'Ascending'}</button><select aria-label="Saved track availability" value={browserState.availability} onchange={event => change({ availability: event.currentTarget.value })}><option value="all">All recordings</option><option value="local">On this device</option><option value="not-local">Not on this device</option></select><input aria-label="Minimum saved track duration" placeholder="From mm:ss" value={browserState.minDuration} oninput={event => change({ minDuration: event.currentTarget.value })} /><input aria-label="Maximum saved track duration" placeholder="To mm:ss" value={browserState.maxDuration} oninput={event => change({ maxDuration: event.currentTarget.value })} />{:else}<select aria-label={browserState.tab === 'artists' && !browserState.entity ? 'Filter artists by source' : 'Filter albums by source'} value={browserState.entitySource} onchange={event => change({ entitySource: event.currentTarget.value })}><option value="">All sources</option><option value="__metadata__">Metadata only (source unknown)</option><option value="spotify">Spotify</option><option value="musicbrainz">MusicBrainz</option><option value="discogs">Discogs</option><option value="local">Local files</option><option value="youtube">YouTube</option><option value="bandcamp">Bandcamp</option><option value="deezer">Deezer</option><option value="monochrome">Monochrome</option><option value="radio_song">Radio metadata</option></select>{#if browserState.tab !== 'artists' || browserState.entity}<input aria-label="Filter albums by year" inputmode="numeric" maxlength="4" placeholder="Year (YYYY)" value={browserState.entityYear} oninput={event => change({ entityYear: event.currentTarget.value.replace(/\D/g, '').slice(0, 4) })} />{/if}<select aria-label="Sort saved albums and artists" value={browserState.entitySort} onchange={event => change({ entitySort: event.currentTarget.value as LibraryBrowserState['entitySort'] })}><option value="name">Name</option><option value="artist">Artist</option><option value="year">Year</option><option value="source">Source</option><option value="tracks">Saved tracks</option></select><button aria-label="Toggle album and artist sort direction" onclick={() => change({ entityDescending: !browserState.entityDescending })}>{browserState.entityDescending ? 'Descending' : 'Ascending'}</button>{/if}
    {#if !trackView}<div class="layout-toggle" role="group" aria-label="Library layout"><button aria-label={`View ${layoutLabel} as a list`} aria-pressed={entityLayout === 'list'} onclick={() => setEntityLayout('list')}><span aria-hidden="true">☰</span> List</button><button aria-label={`View ${layoutLabel} as a grid`} aria-pressed={entityLayout === 'grid'} onclick={() => setEntityLayout('grid')}><span aria-hidden="true">▦</span> Grid</button></div>{/if}
  </div>{/if}
  {#if !albumDetailView}
    {#if notice}<p role="status">{notice}</p>{/if}
    {#if error}<p role="alert">{error}</p><button onclick={() => void load({ ...browserState })}>Retry</button>{/if}
    {#if trackActionError}<p role="alert">{trackActionError}</p>{/if}
    {#if trackActionPending}<p role="status">Loading all matching saved tracks before {trackActionPending === 'play' ? 'playback' : 'queueing'}…</p>{/if}
    {#if loading}<p role="status">Loading saved music…</p>{/if}
    {#if !loading && !error && !total}<p>No saved music here yet.{#if browserState.entity} Browse the catalog to find music to save.{/if}</p>{/if}
  {/if}
  {#if albumDetailView}
    {@const album = browserState.entity!}
    {@const fullySaved = album.totalTracks != null && album.trackCount === album.totalTracks}
    {@const providerLabel = album.provider ? `${album.provider[0]!.toUpperCase()}${album.provider.slice(1)}` : 'Saved library'}
    <AlbumDetailView
      metadata={{ title: album.name, artist: album.artist, year: album.releaseDate?.slice(0, 4), artworkUrl: album.artworkUrl, sourceLabel: providerLabel, sourceUrl: album.sourceUrl, scopeLabel: album.explicitlySaved ? 'Saved album' : 'Saved tracks from catalog album', trackCountLabel: `${album.trackCount} saved tracks${total !== album.trackCount ? ` · ${tracks.length} of ${total} matching tracks loaded` : ` · ${tracks.length} loaded`}${album.totalTracks != null ? ` · ${album.trackCount} of ${album.totalTracks} catalog tracks saved` : ''}` }}
      albumTracks={tracks} actionTrackCount={album.trackCount} resolveAlbumTracks={resolveAlbumTracks} analysisScopeComplete={fullySaved} analysisIdentity={`${album.id}:${album.trackCount}:${album.totalTracks ?? 'unknown'}:${scopeRevision}`}
      ownsScroll={false} loading={loading} visibleTrackCount={total} loadingMessage="Loading saved tracks…" emptyMessage="No saved tracks match this filter. Album actions still use the full saved scope."
      playAlbumLabel={fullySaved ? '▶ Play album' : '▶ Play saved tracks'} queueAlbumLabel={fullySaved ? '＋ Queue album' : '＋ Queue saved tracks'} downloadAlbumLabel={fullySaved ? 'Download album' : 'Download saved tracks'}
      onPlayAlbum={(items, resource) => onPlay(items[0]!, items, 0, resource)} onQueueAlbum={(items, resource) => onQueue(items, 'append', resource)}
      onDownloadAlbum={onDownloadAlbum} onSearchAlbumSoulseek={() => onSearchAlbumSoulseek?.(album)} soulseekSearchAvailable={soulseekSearchAvailable}
      onAddAlbumToPlaylist={onAddAlbumToPlaylist} onAnalyzeLoudness={fullySaved ? analyzeCompleteSavedAlbum : undefined}
      filterValue={browserState.query} onFilterChange={query => change({ query })} sortValue={browserState.sort} sortOptions={[{ value: 'sourceOrder', label: 'Disc / track order' }, { value: 'title', label: 'Title' }, { value: 'artist', label: 'Artist' }, { value: 'album', label: 'Album' }, { value: 'releaseDate', label: 'Release date' }, { value: 'duration', label: 'Duration' }]} onSortChange={sort => change({ sort })}
      sortDirection={browserState.descending ? 'desc' : 'asc'} onDirectionChange={direction => change({ descending: direction === 'desc' })} availabilityValue={browserState.availability} availabilityOptions={[{ value: 'all', label: 'All recordings' }, { value: 'local', label: 'On this device' }, { value: 'not-local', label: 'Not on this device' }]} onAvailabilityChange={availability => change({ availability })}
      minDuration={browserState.minDuration} maxDuration={browserState.maxDuration} onDurationChange={(minDuration, maxDuration) => change({ minDuration, maxDuration })}
      trackListProps={{ tracks, position: (_, index) => index + 1, onPlay, onDownload, onFavorite, onPlaylist, onArtist, onAlbum, onContext, onAddSelectedToPlaylist, onQueue: track => onQueue([track], 'append'), onNext: track => onQueue([track], 'next'), playingTrack, bufferingTrack, bufferingTrackId, playbackFailed, paused, listLabel: 'Saved album tracks', selectionContext: `library:${album.id}` }}
    >
      {#snippet headerActions()}<button onclick={() => onBrowse(album)}>Browse catalog</button>{#if localAlbumIds.length}<button onclick={() => showLocalMatch = true}>Match local files</button>{/if}{/snippet}
      {#snippet moreActions()}<p>Album actions use the complete saved track scope, in disc and track order.</p>{#if !fullySaved}<p>Album loudness analysis is available after every catalog track is saved. {#if album.totalTracks != null}{album.trackCount} of {album.totalTracks} catalog tracks are saved.{:else}The catalog track count is unknown, so completeness cannot be confirmed.{/if}</p>{/if}{#if album.explicitlySaved}<button onclick={() => void removeSave()}>Remove album save</button>{/if}{/snippet}
      {#snippet notices()}{#if notice}<p role="status">{notice}</p>{/if}{#if error}<p role="alert">{error}</p><button onclick={() => void load({ ...browserState })}>Retry</button>{/if}{#if trackActionError}<p role="alert">{trackActionError}</p>{/if}{#if loading}<p role="status">Loading saved music…</p>{/if}{/snippet}
    </AlbumDetailView>
  {:else if trackView}
    <div class="controls"><button disabled={!total || Boolean(trackActionPending)} onclick={() => void runTrackAction('play')}>Play saved tracks</button><button disabled={!total || Boolean(trackActionPending)} onclick={() => void runTrackAction('queue')}>Queue saved tracks</button><small>{tracks.length} of {total} saved tracks loaded</small></div>
    <TrackList presentation="playlist" position={(_, index) => index + 1} {tracks} {onPlay} {onDownload} {onFavorite} {onPlaylist} {onArtist} {onAlbum} {onContext} {onAddSelectedToPlaylist} onQueue={track => onQueue([track], 'append')} onNext={track => onQueue([track], 'next')} {playingTrack} {bufferingTrack} {bufferingTrackId} {playbackFailed} {paused} listLabel="Saved tracks" selectionContext={`library:${browserState.entity?.id}`} />
  {:else}<small>{items.length} of {total} saved {browserState.entity ? 'albums' : browserState.tab === 'artists' ? 'artists' : 'albums'} · {entityGroups.length} cards</small><div class="entity-grid" class:entity-list={entityLayout === 'list'}>{#each entityGroups as group (group.key)}{@const entry = group.entries[0]}{#if entry.kind === 'album' && group.entries.length > 1}<article class="entity-card album-family"><div class="card-art"><ArtworkImage urls={entry.artworkUrl ? [entry.artworkUrl] : []} /></div><strong>{entry.name}</strong><span>{entry.artist}</span><small>{group.entries.length} versions · {[...new Set(group.entries.map(version => version.provider ?? 'Unknown source'))].length} sources</small><details><summary>Choose a version</summary><div class="family-versions">{#each group.entries as version (version.id)}<button class="version-choice" aria-label={`Open ${version.name}, ${version.artist}, ${version.releaseDate ?? 'unknown year'}, ${version.provider ?? 'unknown source'}`} onclick={() => open(version)}><strong>{version.provider ?? 'Unknown source'}{version.providerId ? ` · ${version.providerId}` : ''}</strong><small>{version.releaseDate ?? 'Year unknown'} · {version.trackCount} saved tracks{version.totalTracks != null ? ` of ${version.totalTracks}` : ''}</small>{#if version.explicitlySaved}<small>Saved album</small>{/if}</button>{/each}</div></details></article>{:else if entry.kind === 'artist' && group.entries.length > 1}<article class="entity-card artist-family"><div class="card-art"><ArtworkImage urls={entry.artworkUrl ? [entry.artworkUrl] : []} /></div><strong>{entry.name}</strong><small>{group.entries.length} source profiles · {[...new Set(group.entries.map(profile => profile.provider ?? 'Metadata'))].length} sources</small><small>Same display name; profiles remain separate</small><details><summary>Choose a profile</summary><div class="family-versions">{#each group.entries as profile (profile.id)}<button class="version-choice" aria-label={`Open ${profile.name}, ${profile.provider ?? 'metadata'} profile ${profile.providerId ?? profile.id}`} onclick={() => open(profile)}><strong>{profile.provider ?? 'Metadata profile'}{profile.providerId ? ` · ${profile.providerId}${profile.providerId === '0' ? ' · identity unresolved' : ''}` : ''}</strong><small>{profile.trackCount} saved tracks · {profile.albumCount} albums</small>{#if profile.aliases && profile.aliases.length > 1}<small>{profile.aliases.length} linked catalog references</small>{/if}{#if profile.explicitlySaved}<small>Saved artist</small>{/if}</button>{/each}</div></details></article>{:else}<button class="entity-card" onclick={() => open(entry)}><div class="card-art"><ArtworkImage urls={entry.artworkUrl ? [entry.artworkUrl] : []} /></div><strong>{entry.name}</strong>{#if entry.kind === 'album'}<span>{entry.artist}</span><small>{entry.releaseDate?.slice(0,4) ?? ''} · {entry.trackCount}{entry.totalTracks != null ? ` of ${entry.totalTracks}` : ''} tracks saved</small>{:else}<small>{entry.trackCount} saved tracks · {entry.albumCount} albums</small>{/if}{#if entry.explicitlySaved}<small>Saved {entry.kind}</small>{/if}{#if entry.provider}<small>{entry.provider}</small>{/if}</button>{/if}{/each}</div>{/if}
  {#if (trackView ? tracks.length : items.length) < total}<button disabled={loading} onclick={() => { onStateChange({ ...browserState, loadedCount: (trackView ? tracks.length : items.length) + 200 }); void load({ ...browserState }, true); }}>Load more</button>{/if}
</section>
{#if showLocalMatch && localAlbumIds.length}<LocalAlbumMatch {localAlbumIds} {call} onClose={()=>showLocalMatch=false} onChanged={entry=>{showLocalMatch=false;onNavigate({...browserState,entity:entry,scroll:0,loadedCount:200});}} />{/if}

<style>
  .library-browser{min-height:0;flex:1;overflow:auto;padding:1rem;display:flex;flex-direction:column;gap:1rem}.entity-header{display:flex;align-items:center;gap:1rem}.entity-header h2{margin:.3rem 0}.entity-header p{margin:.3rem 0;color:var(--muted)}.entity-art{width:100px;height:100px;flex:none;overflow:hidden;border-radius:8px}.controls,nav{display:flex;flex-wrap:wrap;align-items:center;gap:.5rem}.controls input{min-width:160px}.layout-toggle{display:flex;gap:.25rem}.entity-grid{display:grid;grid-template-columns:repeat(auto-fill,minmax(155px,1fr));gap:1rem}.entity-card{display:flex;flex-direction:column;gap:.4rem;padding:.7rem;text-align:left;min-width:0}.album-family,.artist-family{border:1px solid var(--border);border-radius:8px}.card-art{width:100%;aspect-ratio:1;overflow:hidden;border-radius:6px}.entity-card span,.entity-card small{color:var(--muted);overflow-wrap:anywhere}.family-versions{display:flex;flex-direction:column;gap:.4rem;width:100%;min-width:0;max-width:100%;box-sizing:border-box;margin-top:.5rem}.version-choice{display:flex;flex-direction:column;gap:.25rem;width:100%;min-width:0;max-width:100%;box-sizing:border-box;text-align:left;white-space:normal;overflow-wrap:anywhere}.entity-list{display:flex;flex-direction:column;gap:.4rem}.entity-list .entity-card{width:100%;min-height:4.25rem;flex-direction:row;align-items:center;gap:.75rem;padding:.45rem .65rem}.entity-list .card-art{width:3.25rem;height:3.25rem;aspect-ratio:1;flex:none}.entity-list .entity-card>strong{min-width:9rem;flex:1 1 12rem}.entity-list .entity-card>span,.entity-list .entity-card>small{flex:0 1 auto}.entity-list .album-family,.entity-list .artist-family{flex-wrap:wrap}.entity-list .family-versions{flex:1 1 calc(100% - 4rem);width:calc(100% - 4rem);min-width:0;max-width:100%;box-sizing:border-box;margin:.2rem 0 0 4rem}.entity-list .version-choice{max-width:100%}button[aria-pressed="true"]{border-color:var(--accent);color:var(--accent)}
</style>
