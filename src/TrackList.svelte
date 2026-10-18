<script lang="ts">
  import { getContext, onMount } from "svelte";
  import { SOURCE_OPENER, type SourceOpener } from "./lib/source-opening";
  import { call, desktop } from "./lib/api";
  import type { Track } from "./lib/types";
  import { positionContextMenu } from "./lib/menu-geometry";
  import MenuIcon from "./MenuIcon.svelte";
  import { sameTrackIdentity } from "./lib/track-identity";
  import { playableDuration } from "./lib/track-sort";
  import { rowWindow } from "./lib/table";

  let {
    tracks, orderedRows, onPlay, onDownload, onFavorite, onNext, onQueue, onPlaylist, onArtist, onInfo,
    onContext, onSelect, onAddSelectedToPlaylist, selectionContext, selectionKeys, selectionTracks, selectionTrackKeys, selectedTrackIds = [], position, meta, timestamp, playingTrack, bufferingTrack, playingTrackId = "", bufferingTrackId = "", paused = false, listLabel = "Tracks", initialScroll, onScroll,
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
    onInfo?: (track: Track) => void | Promise<void>;
    onContext?: (track: Track, event: MouseEvent) => void;
    onSelect?: (track: Track, event: MouseEvent) => void;
    onAddSelectedToPlaylist?: (tracks: Track[]) => void | Promise<void>;
    selectionContext?: string;
    selectionKeys?: string[];
    selectionTracks?: Track[];
    selectionTrackKeys?: string[];
    selectedTrackIds?: string[];
    position?: (track: Track, index: number) => string | number | null | undefined;
    meta?: (track: Track, index: number) => string | null | undefined;
    timestamp?: (track: Track, index: number) => { label: string; full?: string } | null | undefined;
    playingTrack?: Track | null;
    bufferingTrack?: Track | null;
    playingTrackId?: string;
    bufferingTrackId?: string;
    paused?: boolean;
    listLabel?: string;
    initialScroll?: number;
    onScroll?: (scrollTop: number) => void;
  } = $props();
  let openMoreMenu: HTMLDetailsElement | null = null;
  let scrollTop = $state(0);
  let viewportHeight = $state(560);
  let scrollElement: HTMLDivElement;
  const rowHeight = 88;
  const displayRows = $derived(orderedRows ?? tracks.map((track, trackIndex) => ({ kind: "track" as const, key: `${track.id}:${trackIndex}`, trackIndex })));
  const virtualized = $derived(displayRows.length > 200);
  const visibleRange = $derived(virtualized
    ? rowWindow(displayRows.length, scrollTop, viewportHeight, rowHeight, 6, 200)
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
  $effect(() => {
    if (initialScroll === undefined) return;
    const target = initialScroll;
    if (scrollElement && scrollElement.scrollTop !== target) {
      scrollElement.scrollTop = target;
      scrollTop = target;
      viewportHeight = scrollElement.clientHeight || 560;
    }
  });

  function closeOpenMenu(returnFocus = false) {
    const active = openMoreMenu?.open ? openMoreMenu : document.querySelector<HTMLDetailsElement>("details.more[open]");
    if (active) {
      active.open = false;
      if (returnFocus) active.querySelector<HTMLElement>("summary")?.focus();
    }
    openMoreMenu = null;
  }
  function dismissMenuOutside(event: PointerEvent) {
    const active = openMoreMenu?.open ? openMoreMenu : document.querySelector<HTMLDetailsElement>("details.more[open]");
    if (active && event.target instanceof Node && !active.contains(event.target)) closeOpenMenu();
  }
  function dismissMenuEscape(event: KeyboardEvent) {
    if (event.key === "Escape" && (openMoreMenu?.open || document.querySelector("details.more[open]"))) {
      event.preventDefault();
      closeOpenMenu(true);
    }
  }
  function dismissMenuOnScroll(event: Event) {
    if (openMoreMenu && event.target instanceof Node && openMoreMenu.contains(event.target)) return;
    closeOpenMenu();
  }
  onMount(() => {
    window.addEventListener("pointerdown", dismissMenuOutside);
    window.addEventListener("keydown", dismissMenuEscape);
    window.addEventListener("scroll", dismissMenuOnScroll, true);
    const dismissMenuOnResize = () => closeOpenMenu();
    window.addEventListener("resize", dismissMenuOnResize);
    return () => {
      window.removeEventListener("pointerdown", dismissMenuOutside);
      window.removeEventListener("keydown", dismissMenuEscape);
      window.removeEventListener("scroll", dismissMenuOnScroll, true);
      window.removeEventListener("resize", dismissMenuOnResize);
    };
  });

  function context(track: Track, event: MouseEvent) {
    if (!onContext) return;
    event.preventDefault();
    event.stopPropagation();
    onContext(track, event);
  }
  function positionMoreMenu(event: Event) {
    const details = event.currentTarget as HTMLDetailsElement;
    const menu = details.querySelector<HTMLElement>(".more-menu");
    if (!menu) return;
    if (!details.open) {
      menu.style.removeProperty("left");
      menu.style.removeProperty("top");
      if (openMoreMenu === details) openMoreMenu = null;
      return;
    }
    document.querySelectorAll<HTMLDetailsElement>(".shared-track-list details.more[open]").forEach((other) => {
      if (other !== details) other.open = false;
    });
    openMoreMenu = details;
    const summary = details.querySelector("summary");
    if (!summary) return;
    const anchor = summary.getBoundingClientRect();
    const box = menu.getBoundingClientRect();
    const computedPlayerHeight = Number.parseFloat(getComputedStyle(menu).getPropertyValue("--player-height"));
    const playerHeight = Number.isFinite(computedPlayerHeight) ? computedPlayerHeight : 90;
    const { left, top } = positionContextMenu(
      anchor,
      { width: box.width || menu.scrollWidth || 200, height: menu.scrollHeight || box.height || 240 },
      { width: window.innerWidth, height: window.innerHeight },
      playerHeight,
    );
    menu.style.position = "fixed";
    menu.style.left = `${left}px`;
    menu.style.top = `${top}px`;
  }
  const sourceOpener = getContext<SourceOpener | undefined>(SOURCE_OPENER);
  let sourceOpenFailure = $state("");
  async function openTrackSource(event: MouseEvent, track: Track) {
    if ((!sourceOpener && !desktop()) || (event.type === "click" ? event.button !== 0 : event.button !== 1)) return;
    event.preventDefault();
    event.stopPropagation();
    sourceOpenFailure = "";
    runMenuAction(event, () => {});
    try { if (sourceOpener) await sourceOpener(track, track.sourceUrl); else await call("open_external_url", {url:track.sourceUrl}); }
    catch { sourceOpenFailure = "Could not open the recording source."; }
  }
  function runMenuAction(event: MouseEvent, action: () => unknown) {
    void action();
    const details = (event.currentTarget as HTMLElement).closest("details");
    if (openMoreMenu === details) openMoreMenu = null;
    details?.removeAttribute("open");
  }
</script>

<div class:virtualized class="track-list-viewport" bind:this={scrollElement} onscroll={updateViewport}>
{#if sourceOpenFailure}<p role="alert">{sourceOpenFailure}</p>{/if}
{#if onAddSelectedToPlaylist || onSelect}<div class="playlist-selection-actions" role="group" aria-label="Track selection">
  {#if selectionMode}
    <span>{onAddSelectedToPlaylist ? (selectedPlaylistTracks.length ? `${selectedPlaylistTracks.length} tracks selected` : `${tracks.length} visible tracks`) : `${selectedTrackIds.length} tracks selected`}</span>
    {#if onAddSelectedToPlaylist}<button type="button" onclick={selectAllPlaylistRows} disabled={!tracks.length}>Select visible</button>{/if}
    {#if selectedPlaylistTracks.length}<button type="button" onclick={() => selectedForPlaylist = []}>Clear selection</button><button type="button" class="add-selected" onclick={() => void onAddSelectedToPlaylist?.(selectedPlaylistTracks)}>Add selected to playlist</button>{/if}
    <button type="button" class="cancel-selection" onclick={cancelPlaylistSelection}>Cancel</button>
  {:else}
    <button type="button" class="start-selection" aria-label="Select tracks" onclick={() => selectionMode = true}>Select</button>
  {/if}
</div>{/if}
<ol class="shared-track-list" aria-label={listLabel}>
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
    <li class:current class:buffering class:paused={current && paused} class:selected={selectedTrackIds.includes(track.id)} aria-current={current ? "true" : undefined} oncontextmenu={(event) => context(track, event)}>
      {#if selectionMode && (onSelect || onAddSelectedToPlaylist)}<input class="select-track" type="checkbox" checked={selectedTrackIds.includes(track.id) || selectedForPlaylist.includes(selectionKey(track,index))} aria-label={onAddSelectedToPlaylist ? `Select ${track.title} for playlist` : `Select ${track.title}`} onclick={(event) => { event.stopPropagation(); if (onSelect) onSelect(track, event); }} onchange={(event) => { event.stopPropagation(); if (onAddSelectedToPlaylist) togglePlaylistSelection(selectionKey(track,index)); }} />{/if}
      {#if position}<span class="track-position">{position(track, index) ?? index + 1}</span>{/if}
      {#if track.artworkUrl}<img class="artwork" src={track.artworkUrl} alt="" loading="lazy" />{:else}<span class="artwork placeholder" aria-hidden="true">♫</span>{/if}
      {#if current}<span class="playback-state" class:status-buffering={buffering} role="status" aria-label={`${buffering ? "Buffering" : paused ? "Paused" : "Playing"} ${track.title}`} title={`${buffering ? "Buffering" : paused ? "Paused" : "Playing"} · ${track.title}`} aria-live="polite">{buffering ? "…" : paused ? "Ⅱ" : "♫"}</span>{/if}
      <div class="track-copy">
        <button class="track-title" aria-label={`Play ${track.title}`} title={track.title} onclick={() => onPlay?.(track, tracks, index)}>
          <strong>{track.title}{#if track.version}<small>{" "}({track.version})</small>{/if}</strong>
        </button>
        <div class="track-subtitle">
          {#if onArtist}<button class="artist-link" onclick={() => onArtist?.(track)}>{track.artist}</button>{:else}<span>{track.artist}</span>{/if}
          {#if track.album}<span class="album"> · {track.album}</span>{/if}
          {#if meta?.(track, index)}<span> · {meta(track, index)}</span>{/if}
        </div>
        {#if timestamp?.(track, index)}<time class="track-time" title={timestamp(track, index)?.full}>{timestamp(track, index)?.label}</time>{/if}
      </div>
      {#if duration !== null}<span class="duration">{Math.floor(duration / 60)}:{String(Math.floor(duration % 60)).padStart(2, "0")}</span>{/if}
      <div class="row-actions">
        {#if onPlay}<button class="icon-action play" aria-label={`Play ${track.title}`} title="Play" onclick={() => onPlay?.(track, tracks, index)}>▶</button>{/if}
        {#if onFavorite}<button class="icon-action favorite" class:active={track.favorite} aria-label={`${track.favorite ? "Remove" : "Add"} ${track.title} ${track.favorite ? "from" : "to"} favorites`} aria-pressed={track.favorite} title={track.favorite ? "Remove from favorites" : "Add to favorites"} onclick={() => onFavorite?.(track)}>{track.favorite ? "♥" : "♡"}</button>{/if}
        {#if onDownload}<button class="icon-action" aria-label={`Download ${track.title}`} title="Download" onclick={() => onDownload?.(track)}>↓</button>{/if}
        {#if onContext || onNext || onQueue || onPlaylist || onArtist || onInfo || track.sourceUrl}
          <details class="more" ontoggle={positionMoreMenu}><summary aria-label={`More actions for ${track.title}`} title="More actions">⋯</summary><div class="more-menu">
            {#if onNext}<button onclick={(event) => runMenuAction(event, () => onNext?.(track))}><MenuIcon name="play"/><span>Play next</span></button>{/if}
            {#if onQueue}<button onclick={(event) => runMenuAction(event, () => onQueue?.(track))}><MenuIcon name="queue"/><span>Add to queue</span></button>{/if}
            {#if onPlaylist}<button class="primary-action" onclick={(event) => runMenuAction(event, () => onPlaylist?.(track))}><MenuIcon name="plus"/><span>Add to playlist</span></button>{/if}
            {#if onArtist}<button class="menu-section" onclick={(event) => runMenuAction(event, () => onArtist?.(track))}><MenuIcon name="search"/><span>Open artist</span></button>{/if}
            {#if onInfo}<button class:menu-section={!onArtist} onclick={(event) => runMenuAction(event, () => onInfo?.(track))}><MenuIcon name="info"/><span>Track information</span></button>{/if}
            {#if onFavorite}<button class="overflow-favorite menu-section" aria-pressed={track.favorite} onclick={(event) => runMenuAction(event, () => onFavorite?.(track))}><MenuIcon name="heart"/><span>{track.favorite ? "Remove from favorites" : "Add to favorites"}</span></button>{/if}
            {#if onDownload}<button class="overflow-download" onclick={(event) => runMenuAction(event, () => onDownload?.(track))}><MenuIcon name="download"/><span>Download options…</span></button>{/if}
            {#if track.sourceUrl}<a class="menu-section source-action" href={track.sourceUrl} target="_blank" rel="noreferrer" onclick={(event) => void openTrackSource(event, track)} onauxclick={(event) => void openTrackSource(event, track)}><MenuIcon name="external"/><span>Open source</span></a>{/if}
            {#if onContext}<button class="opens-dialog" aria-haspopup="dialog" class:menu-section={Boolean(onNext || onQueue || onPlaylist || onArtist || onInfo || onFavorite || onDownload || track.sourceUrl)} onclick={(event) => runMenuAction(event, () => onContext?.(track, event))}><MenuIcon name="window"/><span>More actions…</span></button>{/if}
          </div></details>
        {/if}
      </div>
    </li>
    {/if}
    {/if}
  {/each}
  {#if virtualized && visibleRange.end < displayRows.length}<li class="virtual-spacer" aria-hidden="true" style={`height:${(displayRows.length - visibleRange.end) * rowHeight}px`}></li>{/if}
</ol>
</div>

<style>
  .track-list-viewport{min-width:0}.track-list-viewport.virtualized{max-height:70vh;overflow:auto;overscroll-behavior:contain;scrollbar-gutter:stable}.track-list-viewport.virtualized .shared-track-list li:not(.virtual-spacer){box-sizing:border-box;height:88px;max-height:88px;overflow:hidden}.track-list-viewport.virtualized .track-subtitle{display:-webkit-box;-webkit-box-orient:vertical;-webkit-line-clamp:2;line-clamp:2;white-space:normal;overflow:hidden;text-overflow:ellipsis}.track-list-viewport.virtualized .track-time{overflow:hidden;text-overflow:ellipsis;white-space:nowrap}.track-list-viewport .virtual-spacer{display:block;padding:0!important;border:0!important;pointer-events:none}
  .shared-track-list{list-style:none;margin:0;padding:0;min-width:0;container-type:inline-size}.shared-track-list li{display:flex;align-items:center;gap:12px;min-width:0;padding:8px 4px;border-bottom:1px solid var(--line);transition:background 120ms ease}.shared-track-list li:hover{background:color-mix(in srgb,var(--raised) 72%,transparent)}.shared-track-list li.current{background:color-mix(in srgb,var(--accent) 12%,transparent)}.shared-track-list li.buffering .icon-action.play{animation:track-pulse 1s ease-in-out infinite alternate}.shared-track-list li.paused .icon-action.play{animation:none}@keyframes track-pulse{to{opacity:.45;transform:scale(.88)}}.track-position{flex:0 0 24px;text-align:right;color:var(--muted);font-size:12px}.playback-state{flex:0 0 22px;text-align:center;color:var(--accent);font-size:15px;font-weight:700}.playback-state.status-buffering{animation:track-pulse 1s ease-in-out infinite alternate}.artwork{flex:0 0 42px;width:42px;height:42px;object-fit:cover;border-radius:5px;background:var(--raised)}.placeholder{display:grid;place-items:center;color:var(--muted)}.track-copy{display:flex;flex:1;flex-direction:column;gap:3px;min-width:0}.track-title{display:block;max-width:100%;min-width:0;padding:0;border:0;background:transparent;color:var(--text);text-align:left}.track-title strong{display:block;overflow:hidden;text-overflow:ellipsis;white-space:nowrap;font-size:13px;font-weight:600}.track-title small{color:var(--muted);font-weight:400}.track-subtitle{overflow:hidden;text-overflow:ellipsis;white-space:nowrap;color:var(--muted);font-size:11px}.artist-link{padding:0;border:0;background:transparent;color:inherit;text-decoration:underline;text-underline-offset:2px}.track-time{color:var(--muted);font-size:10px}.duration{flex:0 0 38px;text-align:right;color:var(--muted);font-size:11px}.row-actions{display:flex;flex:0 0 auto;align-items:center;gap:4px}.icon-action,.more summary{display:grid;place-items:center;width:36px;height:36px;padding:0;border:1px solid var(--line);border-radius:50%;background:transparent;color:var(--muted);font-size:18px;cursor:pointer;touch-action:manipulation}.icon-action:hover,.more summary:hover{color:var(--accent);border-color:var(--accent)}.icon-action.play{color:var(--accent)}.icon-action.favorite.active{color:#f1768d}.more{position:relative}.more summary{list-style:none}.more summary::-webkit-details-marker{display:none}.more-menu{position:fixed;z-index:80;top:8px;left:8px;right:auto;display:flex;flex-direction:column;box-sizing:border-box;width:max-content;min-width:160px;max-width:min(240px,calc(100vw - 16px));max-height:calc(100dvh - var(--player-height,90px) - 16px);overflow:auto;padding:5px;border:1px solid var(--line);border-radius:8px;background:var(--surface);box-shadow:0 10px 30px #0008}.more-menu button,.more-menu a{display:flex;flex:0 0 auto;align-items:center;gap:10px;justify-content:flex-start;box-sizing:border-box;width:100%;min-width:0;height:auto;min-height:36px;padding:5px 9px;border:0;border-radius:5px;background:transparent;color:var(--text);font:inherit;font-size:13px;line-height:1.25;text-align:left;text-decoration:none;white-space:normal;overflow-wrap:anywhere;word-break:normal;touch-action:manipulation}.more-menu button:hover,.more-menu a:hover{background:var(--raised)}.more-menu button.menu-section,.more-menu a.menu-section{margin-top:5px;padding-top:7px;border-top:1px solid var(--line);border-top-left-radius:0;border-top-right-radius:0}.more-menu button.primary-action{margin:3px 0;background:color-mix(in srgb,var(--accent) 18%,var(--surface));font-weight:650}.more-menu button.primary-action:hover{background:color-mix(in srgb,var(--accent) 28%,var(--surface))}.shared-track-list :global(.more-menu svg[aria-hidden="true"]){display:block;flex:0 0 20px;color:var(--muted)}.shared-track-list :global(.more-menu button.primary-action svg[aria-hidden="true"]){color:var(--accent)}.more-menu button:focus-visible,.more-menu a:focus-visible,.icon-action:focus-visible,.more summary:focus-visible{outline:2px solid var(--accent);outline-offset:2px}
  @media(max-width:640px){.shared-track-list li{gap:7px}.track-position{flex-basis:18px;font-size:10px}.artwork{flex-basis:36px;width:36px;height:36px}.duration{display:none}.track-title strong{font-size:12px}.track-subtitle{font-size:10px}.row-actions{gap:2px}}
  @media(max-width:450px){.artwork{flex-basis:32px;width:32px;height:32px}.shared-track-list li{gap:5px}.icon-action,.more summary{font-size:12px}}
  @container (max-width: 560px){.shared-track-list li{gap:7px}.track-title strong,.track-subtitle{overflow:visible;text-overflow:clip;white-space:normal;overflow-wrap:anywhere}.track-title strong{line-height:1.35}.track-subtitle{line-height:1.35}.duration{display:none}}
  @container (max-width: 430px){.shared-track-list li{gap:5px}.track-position,.artwork{display:none}.row-actions>.icon-action{display:none}.more summary{width:40px;height:40px}.row-actions{gap:2px}.track-copy{flex-basis:0}}
  @container (max-width: 350px){.row-actions{gap:1px}.track-title strong{font-size:12px}}
  @media(prefers-reduced-motion:reduce){.shared-track-list li.buffering .icon-action.play{animation:none}}
  .shared-track-list li.selected{outline:1px solid color-mix(in srgb,var(--accent) 45%,transparent);outline-offset:-1px}.select-track{flex:0 0 16px;width:16px;height:16px;margin:0;accent-color:var(--accent)}
  .icon-action{font-size:19px}.more summary{font-size:23px}
  .playlist-selection-actions{display:flex;align-items:center;gap:.45rem;flex-wrap:wrap;padding:.35rem .4rem;margin-bottom:.35rem;border-bottom:1px solid var(--border,#383d43);font-size:.8rem;color:var(--muted,#a3a8ae)}.playlist-selection-actions span{margin-right:auto}.playlist-selection-actions button{font:inherit;color:var(--text,#eee);background:var(--surface,#202328);border:1px solid var(--border,#454a50);border-radius:6px;padding:.3rem .5rem;cursor:pointer}.playlist-selection-actions button:disabled{opacity:.5;cursor:default}.playlist-selection-actions .add-selected{background:var(--accent,#4c78c8);color:#fff;border-color:var(--accent,#4c78c8)}

  @media (min-width:601px) {
    .more-menu { padding:5px;border-radius:6px;border-color:var(--context-menu-border,var(--line));background:var(--context-menu-surface,var(--surface));color:var(--context-menu-text,var(--text));box-shadow:0 8px 28px #0007; }
    .more-menu button,.more-menu a { min-height:32px;gap:10px;padding:4px 8px;border-radius:3px;color:var(--context-menu-text,var(--text));line-height:1.3; }
    .more-menu button:hover,.more-menu a:hover { background:var(--context-menu-hover,var(--raised)); }
    .more-menu button.primary-action { margin:1px 0;background:var(--context-menu-primary,var(--raised));font-weight:600; }
    .more-menu button.primary-action:hover { background:var(--context-menu-hover,var(--raised)); }
    .shared-track-list :global(.more-menu svg[aria-hidden="true"]) { width:18px;height:18px;flex-basis:18px;color:var(--context-menu-muted,var(--muted)); }
    .more-menu button.menu-section,.more-menu a.menu-section { margin-top:4px;padding-top:7px;border-top-color:var(--context-menu-border,var(--line)); }
  }
  .more-menu button.opens-dialog>span { flex:1; }
  .more-menu button.opens-dialog::after { content:"";flex:0 0 6px;width:6px;height:6px;margin:0 3px 0 12px;border-top:1.5px solid currentColor;border-right:1.5px solid currentColor;transform:rotate(45deg);color:var(--context-menu-muted,var(--muted)); }
</style>
