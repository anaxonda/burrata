<script lang="ts">
  import { getContext } from "svelte";
  import { SOURCE_OPENER, type SourceOpener } from "./lib/source-opening";
  import { call } from "./lib/api";
  import type { Track } from "./lib/types";
  import TrackMenu from "./TrackMenu.svelte";
  import BufferingSpinner from "./BufferingSpinner.svelte";
  import ArtworkImage from "./ArtworkImage.svelte";
  import { trackArtworkSources } from "./lib/catalog-artwork";
  import { trackSource } from "./lib/source-link";
  import { sameTrackIdentity } from "./lib/track-identity";
  import { playableDuration } from "./lib/track-sort";
  import { rowWindow } from "./lib/table";
  import { invertSelection } from "./lib/selection";
  import { playlistPosition } from "./lib/source-position";
  import { playlistDropIndex } from "./lib/playlist-reorder";

  let {
    tracks, orderedRows, onPlay, onDownload, onFavorite, onNext, onQueue, onPlaylist, onArtist, onAlbum, onInfo,
    onContext, onSelect, onRowSelect, onAddSelectedToPlaylist, onInvertSelection, selectionContext, selectionKeys, selectionTracks, selectionTrackKeys, selectedTrackIds = [], selectedTrackId, focusedRowKey, selectionLabel, trackTitle, selectionMode: controlledSelectionMode, position, meta, timestamp, playingTrack, bufferingTrack, playingTrackId = "", bufferingTrackId = "", paused = false, listLabel = "Tracks", initialScroll, onScroll, virtualizationThreshold = 200, presentation = "default", reorderable = false, onReorder,
  }: {
    tracks: Track[];
    orderedRows?: Array<{ kind: "track"; key: string; trackIndex: number } | { kind: "status"; key: string; title: string; detail: string }>;
    onPlay?: (track: Track, tracks: Track[], index: number) => void | Promise<void>;
    onDownload?: (track: Track) => void | Promise<void>;
    onFavorite?: (track: Track) => void | Promise<void>;
    onNext?: (track: Track) => void | Promise<void>;
    onQueue?: (track: Track) => void | Promise<void>;
    onPlaylist?: (track: Track) => void | Promise<void>;
    onArtist?: (track: Track) => void | Promise<void>;
    onAlbum?: (track: Track) => void | Promise<void>;
    onInfo?: (track: Track) => void | Promise<void>;
    onContext?: (track: Track, event: MouseEvent) => void;
    onSelect?: (track: Track, event: MouseEvent, index: number) => void;
    onRowSelect?: (track: Track, event: MouseEvent, index: number) => void;
    onAddSelectedToPlaylist?: (tracks: Track[]) => void | Promise<void>;
    /** Invert parent-controlled selection within the currently listed track occurrences. */
    onInvertSelection?: (keys: string[], tracks: Track[]) => void;
    selectionContext?: string;
    selectionKeys?: string[];
    selectionTracks?: Track[];
    selectionTrackKeys?: string[];
    selectedTrackIds?: string[];
    selectedTrackId?: string;
    /** Stable display-row key to highlight and focus after navigation. */
    focusedRowKey?: string | null;
    selectionLabel?: (track: Track, index: number) => string;
    trackTitle?: (track: Track, index: number) => string;
    selectionMode?: boolean;
    position?: (track: Track, index: number) => string | number | null | undefined;
    meta?: (track: Track, index: number) => string | null | undefined;
    timestamp?: (track: Track, index: number) => { label: string; full?: string } | null | undefined;
    playingTrack?: Track | null;
    bufferingTrack?: Track | null;
    playingTrackId?: string;
    bufferingTrackId?: string;
    paused?: boolean;
    listLabel?: string;
    presentation?: "default" | "playlist";
    /** Enable local playlist drag reorder only when the parent is showing source order without filters. */
    reorderable?: boolean;
    /** Called with zero-based persisted occurrence indices (not track IDs). */
    onReorder?: (from: number, to: number) => void | Promise<void>;
    virtualizationThreshold?: number;
    initialScroll?: number;
    onScroll?: (scrollTop: number) => void;
  } = $props();
  let menu = $state<{track: Track; index: number; event: MouseEvent; anchor: {x: number; y: number}} | null>(null);
  let scrollTop = $state(0);
  let viewportHeight = $state(560);
  let scrollElement: HTMLDivElement;
  let draggedPlaylistPosition = $state<number | null>(null);
  let reorderingPlaylist = $state(false);
  const canReorderPlaylist = $derived.by(() => {
    if (!reorderable || !onReorder) return false;
    const positions = tracks.map(track => playlistPosition(track));
    return positions.every((value): value is number => value !== null) && new Set(positions).size === positions.length;
  });
  const rowHeight = $derived(presentation === "playlist" ? 60 : 88);
  const displayRows = $derived(orderedRows ?? tracks.map((track, trackIndex) => ({ kind: "track" as const, key: `${track.id}:${trackIndex}`, trackIndex })));
  const virtualized = $derived(displayRows.length > virtualizationThreshold);
  const visibleRange = $derived(virtualized
    ? rowWindow(displayRows.length, scrollTop, viewportHeight, rowHeight, 6, virtualizationThreshold)
    : { start: 0, end: displayRows.length });
  const visibleRows = $derived(displayRows.slice(visibleRange.start, visibleRange.end));
  function occurrenceKeys(sourceTracks: Track[]) {
    const counts = new Map<string, number>();
    return sourceTracks.map(track => { const count = counts.get(track.id) ?? 0; counts.set(track.id, count + 1); return `${track.id}:${count}`; });
  }
  const defaultVisibleKeys = $derived(occurrenceKeys(tracks));
  const defaultSelectionSourceKeys = $derived(occurrenceKeys(selectionTracks ?? tracks));
  let selectedForPlaylist = $state<string[]>([]);
  let selectionMode = $state(false);
  const showSelection = $derived(controlledSelectionMode ?? selectionMode);
  let currentSelectionContext = $state<string | undefined>(undefined);
  $effect(() => {
    const context = selectionContext;
    if (context !== currentSelectionContext) { currentSelectionContext = context; selectedForPlaylist = []; selectionMode = false; }
  });
  const selectedPlaylistTracks = $derived.by(() => {
    return selectedForPlaylist.flatMap(key => selectedPlaylistTrackByKey.has(key) ? [selectedPlaylistTrackByKey.get(key)!] : []);
  });
  const selectedPlaylistTrackByKey = $derived.by(() => {
    const sourceTracks = selectionTracks ?? tracks;
    const keys = selectionTrackKeys ?? defaultSelectionSourceKeys;
    const byKey = new Map<string, Track>();
    keys.forEach((key, index) => { const track = sourceTracks[index]; if (track) byKey.set(key, track); });
    return byKey;
  });
  const visibleSelectionPositionByKey = $derived.by(() => {
    const keys = selectionKeys ?? defaultVisibleKeys;
    const positions = new Map<string, number>();
    displayRows.forEach((row, rowIndex) => { if (row.kind === "track" && keys[row.trackIndex] !== undefined) positions.set(keys[row.trackIndex]!, rowIndex); });
    return positions;
  });
  const selectionScopeKeys = $derived(displayRows.flatMap(row => row.kind === "track" && selectionKeys?.[row.trackIndex] !== undefined
    ? [selectionKeys[row.trackIndex]!]
    : row.kind === "track" ? [defaultVisibleKeys[row.trackIndex]!] : []));
  const selectionScopeTracks = $derived(displayRows.flatMap(row => row.kind === "track" && tracks[row.trackIndex] ? [tracks[row.trackIndex]!] : []));
  function selectionKey(track: Track, index: number) {
    if (selectionKeys?.[index] !== undefined) return selectionKeys[index]!;
    return defaultVisibleKeys[index] ?? track.id;
  }
  function togglePlaylistSelection(key: string) {
    if (selectedForPlaylist.includes(key)) { selectedForPlaylist = selectedForPlaylist.filter(item => item !== key); return; }
    const newIndex = visibleSelectionPositionByKey.get(key) ?? -1;
    const insertBefore = selectedForPlaylist.findIndex(item => {
      const position = visibleSelectionPositionByKey.get(item);
      return newIndex >= 0 && position !== undefined && position > newIndex;
    });
    const next = [...selectedForPlaylist];
    next.splice(insertBefore < 0 ? next.length : insertBefore, 0, key);
    selectedForPlaylist = next;
  }
  function selectAllPlaylistRows() {
    const keys = selectionKeys ?? defaultVisibleKeys;
    selectedForPlaylist = displayRows.flatMap(row => row.kind === "track" && keys[row.trackIndex] !== undefined ? [keys[row.trackIndex]!] : []);
  }
  function cancelPlaylistSelection() {
    selectedForPlaylist = [];
    selectionMode = false;
  }
  function updateViewport(event: Event) {
    const viewport = event.currentTarget as HTMLDivElement;
    scrollTop = viewport.scrollTop;
    viewportHeight = viewport.clientHeight || 560;
    onScroll?.(scrollTop);
  }
  function handleRowClick(node: HTMLOListElement) {
    const onClick = (event: MouseEvent) => {
      const target = event.target as Element;
      if (target.closest("button, input, a")) return;
      const row = target.closest<HTMLLIElement>("[data-track-index]");
      if (!row || !node.contains(row)) return;
      const index = Number(row.dataset.trackIndex);
      const track = tracks[index];
      if (track && Number.isInteger(index)) onRowSelect?.(track, event, index);
    };
    node.addEventListener("click", onClick);
    return { destroy: () => node.removeEventListener("click", onClick) };
  }
  $effect(() => {
    if (initialScroll === undefined) return;
    const target = initialScroll;
    if (scrollElement && scrollElement.scrollTop !== target) {
      scrollElement.scrollTop = target;
      scrollTop = target;
      viewportHeight = scrollElement.clientHeight || 560;
    }
  });
  $effect(() => {
    const key = focusedRowKey;
    if (!key || !scrollElement) return;
    const index = displayRows.findIndex(row => row.key === key);
    if (index < 0) return;
    if (virtualized) {
      const target = Math.max(0, index * rowHeight - rowHeight * 2);
      if (scrollElement.scrollTop !== target) scrollElement.scrollTop = target;
      scrollTop = target;
    }
    requestAnimationFrame(() => {
      const row = [...scrollElement.querySelectorAll<HTMLLIElement>("li[data-row-key]")]
        .find(item => item.dataset.rowKey === key);
      row?.querySelector<HTMLButtonElement>("button.track-title")?.focus({ preventScroll: true });
    });
  });

  function openActions(track: Track, index: number, event: MouseEvent) {
    event.preventDefault(); event.stopPropagation();
    if (onContext) { onContext(track, event); return; }
    const bounds = event.type === "contextmenu" ? null : (event.currentTarget as HTMLElement).getBoundingClientRect();
    menu = {track, index, event, anchor: bounds ? {x: bounds.right, y: bounds.bottom} : {x: event.clientX, y: event.clientY}};
  }
  const sourceOpener = getContext<SourceOpener | undefined>(SOURCE_OPENER);
  let sourceOpenFailure = $state("");
  async function openTrackSource(track: Track) {
    const source = trackSource(track);
    if (!source) return;
    sourceOpenFailure = "";
    try { if (sourceOpener) await sourceOpener(track, source.url); else await call("open_external_url", {url:source.url}); }
    catch { sourceOpenFailure = "Could not open the recording source."; }
  }
  function selectMenuTrack(track: Track, index: number, event: MouseEvent) {
    selectionMode = true;
    if (onSelect) onSelect(track, event, index);
    else if (onAddSelectedToPlaylist) togglePlaylistSelection(selectionKey(track, index));
  }

</script>

<div class:virtualized class:playlist={presentation === "playlist"} style:--track-row-height={`${rowHeight}px`} class="track-list-viewport" bind:this={scrollElement} onscroll={updateViewport}>
{#if sourceOpenFailure}<p role="alert">{sourceOpenFailure}</p>{/if}
  {#if onAddSelectedToPlaylist || (onSelect && controlledSelectionMode === undefined) || (onInvertSelection && showSelection)}<div class="playlist-selection-actions" role="group" aria-label="Track selection">
  {#if showSelection}
    <span>{onAddSelectedToPlaylist ? (selectedPlaylistTracks.length ? `${selectedPlaylistTracks.length} tracks selected` : `${tracks.length} visible tracks`) : `${selectedTrackIds.length} tracks selected`}</span>
    {#if onAddSelectedToPlaylist}<button type="button" onclick={selectAllPlaylistRows} disabled={!tracks.length}>Select visible</button>{/if}
    {#if onInvertSelection || onAddSelectedToPlaylist}<button type="button" class="invert-selection" disabled={!selectionScopeKeys.length} onclick={() => onInvertSelection ? onInvertSelection(selectionScopeKeys, selectionScopeTracks) : selectedForPlaylist = invertSelection(selectionScopeKeys, selectedForPlaylist)}>Invert selection</button>{/if}
    {#if selectedPlaylistTracks.length}<button type="button" onclick={() => selectedForPlaylist = []}>Clear selection</button><button type="button" class="add-selected" onclick={() => void onAddSelectedToPlaylist?.(selectedPlaylistTracks)}>Add selected to playlist</button>{/if}
    {#if controlledSelectionMode === undefined}<button type="button" class="cancel-selection" onclick={cancelPlaylistSelection}>Cancel</button>{/if}
  {:else}
    <button type="button" class="start-selection" aria-label="Select tracks" onclick={() => selectionMode = true}>Select</button>
  {/if}
</div>{/if}
<ol class="shared-track-list" aria-label={listLabel} use:handleRowClick>
  {#if virtualized && visibleRange.start > 0}<li class="virtual-spacer" aria-hidden="true" style={`height:${visibleRange.start * rowHeight}px`}></li>{/if}
  {#each visibleRows as displayRow, visibleIndex (displayRow.key)}
    {@const index = displayRow.kind === "track" ? displayRow.trackIndex : -1}
    {#if displayRow.kind === "status"}
      <li class="track-status-row" role="status" aria-label={displayRow.title}><div class="track-copy"><strong>{displayRow.title}</strong><span class="track-subtitle">{displayRow.detail}</span></div></li>
    {:else}
    {@const track = tracks[index]}
    {#if track}
    {@const current = playingTrack ? sameTrackIdentity(track, playingTrack) : track.id === playingTrackId}
    {@const buffering = bufferingTrack ? sameTrackIdentity(track, bufferingTrack) : track.id === bufferingTrackId}
    {@const duration = playableDuration(track)}
    {@const occurrencePosition = playlistPosition(track)}
    <li data-track-index={index} data-row-key={displayRow.key} class:focused-history-occurrence={focusedRowKey === displayRow.key} class:current class:buffering class:paused={current && paused} class:selected={selectedTrackIds.includes(track.id) || selectedTrackId === track.id} class:playlist-drop-target={draggedPlaylistPosition !== null && draggedPlaylistPosition !== occurrencePosition} title={timestamp?.(track, index)?.full} aria-current={focusedRowKey === displayRow.key ? "location" : current ? "true" : undefined} oncontextmenu={(event) => openActions(track, index, event)} ondragover={(event) => { if (canReorderPlaylist && draggedPlaylistPosition !== null && occurrencePosition !== null && draggedPlaylistPosition !== occurrencePosition) { event.preventDefault(); if (event.dataTransfer) event.dataTransfer.dropEffect = "move"; } }} ondrop={(event) => { event.preventDefault(); if (!canReorderPlaylist || draggedPlaylistPosition === null || occurrencePosition === null) return; const fromPosition = draggedPlaylistPosition; const bounds = (event.currentTarget as HTMLElement).getBoundingClientRect(); const after = event.clientY >= bounds.top + bounds.height / 2; const destination = playlistDropIndex(fromPosition, occurrencePosition, after); draggedPlaylistPosition = null; if (destination !== null) { reorderingPlaylist = true; void Promise.resolve(onReorder?.(fromPosition - 1, destination)).finally(() => reorderingPlaylist = false); } }}>
      {#if showSelection && (onSelect || onAddSelectedToPlaylist)}<input class="select-track" type="checkbox" checked={selectedTrackIds.includes(track.id) || selectedTrackIds.includes(selectionKeys?.[index] ?? "") || selectedForPlaylist.includes(selectionKey(track,index))} aria-label={selectionLabel?.(track,index) ?? (onAddSelectedToPlaylist ? `Select ${track.title} for playlist` : `Select ${track.title}`)} onclick={(event) => { event.stopPropagation(); if (onSelect) onSelect(track, event, index); }} onchange={(event) => { event.stopPropagation(); if (onAddSelectedToPlaylist) togglePlaylistSelection(selectionKey(track,index)); }} />{/if}
      {#if canReorderPlaylist}<button type="button" class="playlist-reorder-handle" draggable={!reorderingPlaylist} disabled={reorderingPlaylist} aria-label={`Reorder ${track.title} at position ${occurrencePosition}`} title="Drag to reorder this playlist item" ondragstart={(event) => { if (occurrencePosition === null) { event.preventDefault(); return; } draggedPlaylistPosition = occurrencePosition; event.dataTransfer?.setData("text/plain", "burrata-playlist-row"); if (event.dataTransfer) event.dataTransfer.effectAllowed = "move"; }} ondragend={() => draggedPlaylistPosition = null}>⠿</button>{/if}
      {#if position || presentation === "playlist"}<div class="position-slot"><span class="track-position row-number">{position?.(track, index) ?? index + 1}</span>{#if presentation === "playlist" && onPlay}<button class="position-play row-play" class:buffering aria-busy={buffering} class:paused={current && paused} aria-label={buffering ? `Buffering ${track.title}` : `Play ${track.title}`} title={buffering ? "Buffering audio" : "Play audio"} onclick={() => onPlay?.(track, tracks, index)}>{#if buffering}<BufferingSpinner />{:else}▶{/if}</button>{/if}</div>{/if}
      <span class="artwork placeholder" aria-hidden="true"><ArtworkImage urls={trackArtworkSources(track)} /></span>
      {#if current || buffering && presentation !== "playlist"}<span class="playback-state table-playback-state" class:status-buffering={buffering} class:status-buffering-has-play={buffering && presentation !== "playlist" && Boolean(onPlay)} role="status" aria-label={`${buffering ? "Buffering" : paused ? "Paused" : "Playing"} ${track.title}`} title={`${buffering ? "Buffering" : paused ? "Paused" : "Playing"} · ${track.title}`} aria-live="polite">
        {#if buffering}{#if presentation !== "playlist" || !onPlay}<BufferingSpinner />{/if}{:else if paused}Ⅱ{:else}<svg class="playback-playing-icon" viewBox="0 0 20 20" aria-hidden="true"><path d="M4 11v3M8 8v6M12 5v9M16 9v5"/></svg>{/if}
      </span>{/if}
      <div class="track-copy">
        <button class="track-title" aria-label={`Play ${track.title}`} title={trackTitle?.(track, index) ?? track.title} onclick={(event) => { if (event.shiftKey || event.ctrlKey || event.metaKey) onRowSelect?.(track, event, index); else onPlay?.(track, tracks, index); }}>
          <strong>{track.title}{#if track.version}<small>{" "}({track.version})</small>{/if}</strong>
        </button>
        <div class="track-subtitle">
          {#if track.artist}{#if onArtist}<button class="artist-link" aria-label={`Find recordings by ${track.artist}`} onclick={() => onArtist?.(track)}>{track.artist}</button>{:else}<span>{track.artist}</span>{/if}{/if}
          {#if track.album}<span class="album">{track.artist ? " · " : ""}{#if onAlbum}<button class="album-link" aria-label={`Open album ${track.album}`} onclick={() => onAlbum?.(track)}>{track.album}</button>{:else}{track.album}{/if}</span>{/if}
          {#if meta?.(track, index)}<span>{track.artist || track.album ? " · " : ""}{meta(track, index)}</span>{/if}
        </div>
        {#if presentation !== "playlist" && timestamp?.(track, index)}<time class="track-time" title={timestamp(track, index)?.full}>{timestamp(track, index)?.label}</time>{/if}
      </div>
      {#if presentation === "playlist" && timestamp?.(track, index)}<time class="track-time playlist-time" title={timestamp(track, index)?.full}>{timestamp(track, index)?.label}</time>{/if}
      {#if duration !== null}<span class="duration">{Math.floor(duration / 60)}:{String(Math.floor(duration % 60)).padStart(2, "0")}</span>{/if}
      <div class="row-actions">
        {#if onPlay}<button class="icon-action play" class:buffering={buffering && presentation !== "playlist"} aria-busy={buffering && presentation !== "playlist"} aria-label={buffering && presentation !== "playlist" ? `Buffering ${track.title}` : `Play ${track.title}`} title={buffering && presentation !== "playlist" ? "Buffering audio" : "Play"} onclick={() => onPlay?.(track, tracks, index)}>▶</button>{/if}
        {#if onFavorite}<button class="icon-action favorite" class:active={track.favorite} aria-label={`${track.favorite ? "Remove" : "Add"} ${track.title} ${track.favorite ? "from" : "to"} favorites`} aria-pressed={track.favorite} title={track.favorite ? "Remove from favorites" : "Add to favorites"} onclick={() => onFavorite?.(track)}>{track.favorite ? "♥" : "♡"}</button>{/if}
        {#if onDownload}<button class="icon-action" aria-label={`Download ${track.title}`} title="Download" onclick={() => onDownload?.(track)}>↓</button>{/if}
        {#if onContext || onNext || onQueue || onPlaylist || onArtist || onInfo || onFavorite || onDownload || onSelect || onAddSelectedToPlaylist || trackSource(track)}
          <button class="more-trigger row-more" aria-label={`Actions for ${track.title}`} aria-haspopup="dialog" title="Track actions" onclick={(event) => openActions(track, index, event)}>⋯</button>
        {/if}
      </div>
    </li>
    {/if}
    {/if}
  {/each}
  {#if virtualized && visibleRange.end < displayRows.length}<li class="virtual-spacer" aria-hidden="true" style={`height:${(displayRows.length - visibleRange.end) * rowHeight}px`}></li>{/if}
</ol>
</div>

{#if menu}
  {@const selected = menu}
  <TrackMenu track={selected.track} anchor={selected.anchor} onClose={() => menu = null}
    onNext={onNext ? () => onNext(selected.track) : undefined} onQueue={onQueue ? () => onQueue(selected.track) : undefined}
    onPlaylist={onPlaylist ? () => onPlaylist(selected.track) : undefined} onFavorite={onFavorite ? () => onFavorite(selected.track) : undefined}
    onDownload={onDownload ? () => onDownload(selected.track) : undefined} onDetails={onInfo ? () => onInfo(selected.track) : undefined}
    onArtist={onArtist ? () => onArtist(selected.track) : undefined} onOpenSource={() => void openTrackSource(selected.track)}
    onSelect={onSelect || onAddSelectedToPlaylist ? () => selectMenuTrack(selected.track, selected.index, selected.event) : undefined}
    onGoToAlbum={onAlbum ? () => onAlbum(selected.track) : undefined} onGoToAlbumUnavailable={!onAlbum} />
{/if}

<style>
  .track-list-viewport{min-width:0}.track-list-viewport.virtualized{max-height:70vh;overflow:auto;overscroll-behavior:contain;scrollbar-gutter:stable}.track-list-viewport.virtualized .shared-track-list li:not(.virtual-spacer){box-sizing:border-box;height:var(--track-row-height,88px);max-height:var(--track-row-height,88px);overflow:hidden}.track-list-viewport.virtualized .track-subtitle{display:-webkit-box;-webkit-box-orient:vertical;-webkit-line-clamp:2;line-clamp:2;white-space:normal;overflow:hidden;text-overflow:ellipsis}.track-list-viewport.virtualized .track-time{overflow:hidden;text-overflow:ellipsis;white-space:nowrap}.track-list-viewport .virtual-spacer{display:block;padding:0!important;border:0!important;pointer-events:none}
  .shared-track-list{list-style:none;margin:0;padding:0;min-width:0;container-type:inline-size}.shared-track-list li{display:flex;align-items:center;gap:12px;min-width:0;padding:8px 4px;border-bottom:1px solid var(--line);transition:background 120ms ease}.shared-track-list li:hover{background:color-mix(in srgb,var(--raised) 72%,transparent)}.shared-track-list li.current{background:color-mix(in srgb,var(--accent) 12%,transparent)}.shared-track-list li.focused-history-occurrence{outline:2px solid var(--accent);outline-offset:-2px;background:color-mix(in srgb,var(--accent) 16%,var(--surface))}.track-position{flex:0 0 24px;text-align:right;color:var(--muted);font-size:12px}.playback-state{display:grid;place-items:center;flex:0 0 22px;color:var(--accent);font-size:15px;font-weight:700}.playback-state.status-buffering-has-play{display:none}.artwork{flex:0 0 42px;width:42px;height:42px;object-fit:cover;border-radius:5px;background:var(--raised)}.placeholder{display:grid;place-items:center;color:var(--muted)}.track-copy{display:flex;flex:1;flex-direction:column;gap:3px;min-width:0}.track-title{display:block;max-width:100%;min-width:0;padding:0;border:0;background:transparent;color:var(--text);text-align:left}.track-title strong{display:block;overflow:hidden;text-overflow:ellipsis;white-space:nowrap;font-size:13px;font-weight:600}.track-title small{color:var(--muted);font-weight:400}.track-subtitle{overflow:hidden;text-overflow:ellipsis;white-space:nowrap;color:var(--muted);font-size:11px}.artist-link,.album-link{padding:0;border:0;background:transparent;color:inherit;text-decoration:underline;text-underline-offset:2px;font:inherit;cursor:pointer}.track-time{color:var(--muted);font-size:10px}.duration{flex:0 0 38px;text-align:right;color:var(--muted);font-size:11px}.row-actions{display:flex;flex:0 0 auto;align-items:center;gap:4px}.icon-action,.more-trigger{display:grid;place-items:center;width:36px;height:36px;padding:0;border:1px solid var(--line);border-radius:50%;background:transparent;color:var(--muted);font-size:18px;cursor:pointer;touch-action:manipulation}.icon-action:hover,.more-trigger:hover{color:var(--accent);border-color:var(--accent)}.icon-action.play{color:var(--accent)}.icon-action.favorite.active{color:#f1768d}.icon-action.play.buffering{display:grid;place-items:center;opacity:1}
  .position-slot{position:relative;display:flex;flex:0 0 24px;align-items:center;justify-content:flex-end}.position-play{position:absolute;inset:0;display:none;padding:0;border:0;background:var(--surface);color:var(--accent);cursor:pointer}.playlist .position-play.buffering{display:grid;place-items:center;position:absolute;z-index:1;box-sizing:border-box;opacity:1;border:0;border-radius:50%;background:var(--surface);color:var(--accent)}
  .playlist-reorder-handle{display:grid;flex:0 0 28px;place-items:center;width:28px;height:32px;padding:0;border:0;border-radius:4px;background:transparent;color:var(--muted);font-size:18px;cursor:grab;touch-action:none}.playlist-reorder-handle:active{cursor:grabbing}.playlist-reorder-handle:focus-visible{outline:2px solid var(--accent);outline-offset:2px}.playlist-drop-target{border-top-color:var(--accent)!important}
  @media(min-width:641px){
    .playlist .shared-track-list li:not(.virtual-spacer){position:relative;box-sizing:border-box;height:60px;max-height:60px;gap:12px;padding:7px 10px;overflow:hidden}
    .playlist .artwork{flex-basis:36px;width:36px;height:36px;border-radius:4px}
    .playlist .track-copy{gap:0}.playlist .track-title strong{font-size:12px;line-height:1.5}.playlist .track-subtitle{margin-top:3px;font-size:10px;line-height:1.5;display:block;white-space:nowrap;overflow:hidden;text-overflow:ellipsis}
    .playlist .track-position{font-size:11px}.playlist .position-slot{flex-basis:24px}.playlist li:hover .position-play,.playlist .position-play:focus-visible{display:block}.playlist li:hover .position-slot .track-position{visibility:hidden}
    .playlist li.current .position-slot .track-position{visibility:hidden}.playlist .playback-state{position:absolute;left:10px;width:24px;background:transparent;font-size:12px;pointer-events:none}.playlist li:hover .playback-state{opacity:0}.playlist .row-actions{gap:4px}.playlist .row-actions>.play{display:none}.playlist .icon-action,.playlist .more-trigger{width:28px;height:28px;border:0;border-radius:4px;font-size:17px}
    .playlist .playlist-time{flex:0 1 190px;min-width:0;text-align:right;white-space:nowrap;overflow:hidden;text-overflow:ellipsis;font-size:10px}.playlist .duration{font-size:11px}
  }
  @media(max-width:640px){.shared-track-list li{gap:7px}.track-position{flex-basis:18px;font-size:10px}.artwork{flex-basis:36px;width:36px;height:36px}.duration{display:none}.track-title strong{font-size:12px}.track-subtitle{font-size:10px}.row-actions{gap:2px}.playlist .row-actions>.play{display:none}}
  @media(max-width:450px){.artwork{flex-basis:32px;width:32px;height:32px}.shared-track-list li{gap:5px}.icon-action,.more-trigger{font-size:12px}}
  @container (max-width: 560px){.shared-track-list li{gap:7px}.track-title strong,.track-subtitle{overflow:visible;text-overflow:clip;white-space:normal;overflow-wrap:anywhere}.track-title strong{line-height:1.35}.track-subtitle{line-height:1.35}.duration{display:none}}
  @container (max-width: 560px){.playlist .playlist-time{display:none}.playlist .track-copy{flex:1 1 0;min-width:0}.playlist .track-title strong,.playlist .track-subtitle{display:block;white-space:nowrap;overflow:hidden;text-overflow:ellipsis;line-height:1.5}.playlist .row-actions{flex:0 0 auto}}
  @container (max-width: 430px){.shared-track-list li{gap:5px}.track-position,.artwork{display:none}.playlist .track-position{display:block}.playback-state.status-buffering-has-play{display:grid}.row-actions>.icon-action{display:none}.more-trigger{width:40px;height:40px}.row-actions{gap:2px}.track-copy{flex-basis:0}}
  @container (max-width: 350px){.row-actions{gap:1px}.track-title strong{font-size:12px}}
  .shared-track-list li.selected{outline:1px solid color-mix(in srgb,var(--accent) 45%,transparent);outline-offset:-1px}.select-track{flex:0 0 16px;width:16px;height:16px;margin:0;accent-color:var(--accent)}
  .icon-action{font-size:19px}.more-trigger{font-size:23px}
  .playlist-selection-actions{display:flex;align-items:center;gap:.45rem;flex-wrap:wrap;padding:.35rem .4rem;margin-bottom:.35rem;border-bottom:1px solid var(--border,#383d43);font-size:.8rem;color:var(--muted,#a3a8ae)}.playlist-selection-actions span{margin-right:auto}.playlist-selection-actions button{font:inherit;color:var(--text,#eee);background:var(--surface,#202328);border:1px solid var(--border,#454a50);border-radius:6px;padding:.3rem .5rem;cursor:pointer}.playlist-selection-actions button:disabled{opacity:.5;cursor:default}.playlist-selection-actions .add-selected{background:var(--accent,#4c78c8);color:#fff;border-color:var(--accent,#4c78c8)}

  .icon-action:focus-visible,.more-trigger:focus-visible{outline:2px solid var(--accent);outline-offset:2px}
  .playback-playing-icon{display:block;width:16px;height:16px;fill:none;stroke:currentColor;stroke-width:1.8;stroke-linecap:round;stroke-linejoin:round}
</style>
