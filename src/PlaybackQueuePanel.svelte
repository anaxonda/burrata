<script lang="ts">
  import { planQueueSelectionMove } from "./lib/playback-queue";
  import { trackSortAvailability, trackSortOptionLabel } from "./lib/track-sort-availability";
  import { positionContextMenu } from "./lib/menu-geometry";
  import MenuIcon from "./MenuIcon.svelte";
import { onMount, tick, untrack } from "svelte";
  import type { Chapter, Track } from "./lib/types";
  import { activeChapter, chapterRelativePosition } from "./lib/chapters";
  import type { PlaybackRange, RangedTrack } from "./lib/chapters";
  import { playableDuration, type SortDirection, type TrackSortField } from "./lib/track-sort";

  let {
    tracks,
    currentIndex,
    paused = false,
    buffering = false,
    activePlaybackTrack = null,
    chapterRangesByTrack = {},
    chapters = [],
    position,
    chapterPosition,
    onClose,
    onPlay,
    onMove,
    interactionSignal,
    queueRevision,
    onInteractionMove,
    onSortUpcoming,
    onRemove,
    onSeek,
    onInspectChapters,
    chaptersByTrack = {},
    onPlayChapter,
    onQueueChapter,
    onAppendChapters,
    onDownloadChapter,
    onAddChapterToPlaylist,
    onInfoChapter,
    onQueueAllChapters,
    radioNowPlaying = null,
    radioStationName = "",
    radioHistory = [],
    radioRecognizing = false,
    radioIdentifyProgress = 0,
    onIdentifyCurrent,
    onCancelIdentifyCurrent,
    identifyingCurrentAudio = false,
    onIdentifyRadio,
    onCancelIdentifyRadio,
    onPlayRadioTrack,
    onContextRadioTrack,
    onInfoTrack,
    onAddToPlaylist,
  }: {
    tracks: Track[];
    currentIndex: number;
    paused?: boolean;
    buffering?: boolean;
    activePlaybackTrack?: Track | null;
    chapterRangesByTrack?: Record<string, Record<string, PlaybackRange>>;
    chapters?: Chapter[];
    position: number;
    chapterPosition?: number;
    repeatMode: "off" | "all" | "one";
    shuffleEnabled: boolean;
    onClose: () => void;
    onPlay: (index: number) => void;
    interactionSignal?: AbortSignal;
    queueRevision?: number;
    onInteractionMove?: (from:number,to:number)=>Promise<void>;
    onMove: (from: number, to: number, expectedQueueRevision?:number) => void | {queueRevision?:number} | Promise<void|{queueRevision?:number}>;
    onSortUpcoming?: (field: TrackSortField, direction: SortDirection) => void;
    onRemove: (index: number) => void;
    onSeek: (seconds: number) => void;
    onToggleShuffle: () => void;
    onCycleRepeat: () => void;
    onInspectChapters?: (track: Track) => void;
    chaptersByTrack?: Record<string, Chapter[]>;
    onPlayChapter?: (parent: Track, chapter: Chapter, parentIndex: number) => void;
    onQueueChapter?: (parent: Track, chapter: Chapter, parentIndex: number) => void;
    onAppendChapters?: (parent: Track, chapters: Chapter[]) => void;
    onDownloadChapter?: (parent: Track, chapter: Chapter, parentIndex: number) => void;
    onAddChapterToPlaylist?: (parent: Track, chapter: Chapter, parentIndex: number) => void;
    onInfoChapter?: (parent: Track, chapter: Chapter, parentIndex: number) => void;
    onQueueAllChapters?: (parent: Track, chapters: Chapter[]) => void;
    radioNowPlaying?: Track | null;
    radioStationName?: string;
    radioHistory?: { id: string; track: Track; playedAt: string; stationName: string }[];
    radioRecognizing?: boolean;
    radioIdentifyProgress?: number;
    onIdentifyCurrent?: () => void;
    onCancelIdentifyCurrent?: () => void;
    identifyingCurrentAudio?: boolean;
    onIdentifyRadio?: () => void;
    onCancelIdentifyRadio?: () => void;
    onPlayRadioTrack?: (track: Track) => void;
    onDownloadRadioTrack?: (track: Track) => void;
    onFavoriteRadioTrack?: (track: Track) => void;
    onArtistRadioTrack?: (track: Track) => void;
    onInfoRadioTrack?: (track: Track) => void;
    onContextRadioTrack?: (track: Track) => void;
    onInfoTrack?: (track: Track) => void;
    onAddToPlaylist?: (track: Track, index: number) => void;
  } = $props();

  let panel: HTMLElement;
  let selectedRows = $state<number[]>([]);
  let selectionAnchor = -1;
  let draggedRows: number[] = [];
  let draggedRevision:number|undefined;
  let movingRows = $state(false);
  let moveError = $state("");
  let moveFailureSerial = $state(0);
  $effect(() => { const rows=tracks;const revision=queueRevision; untrack(()=>{if(!movingRows){selectedRows=[];selectionAnchor=-1;draggedRows=[];}}); });
  function selectQueueRow(index: number, event: MouseEvent) {
    if (movingRows) return;
    if (event.shiftKey && selectionAnchor >= 0) {
      const low=Math.min(index,selectionAnchor),high=Math.max(index,selectionAnchor);
      selectedRows=Array.from({length:high-low+1},(_,at)=>low+at);
    } else if (event.ctrlKey || event.metaKey) {
      selectedRows=selectedRows.includes(index)?selectedRows.filter(at=>at!==index):[...selectedRows,index].sort((a,b)=>a-b);
      selectionAnchor=index;
    } else { selectedRows=[index]; selectionAnchor=index; }
  }
  async function moveSelection(indices: number[], target: number, focusOrigin?: HTMLElement, synthetic=false,baseRevision=queueRevision) {
    if(movingRows)return;
    const plan=planQueueSelectionMove(tracks.length,indices,target);
    const signal=synthetic?interactionSignal:undefined;
    if(signal?.aborted)return;
    let expectedRevision=baseRevision;
    movingRows=true;moveError="";
    try { for(const [from,to] of plan.moves) { if(signal?.aborted)throw new Error("Queue interaction cancelled");if(signal&&onInteractionMove)await onInteractionMove(from,to);else {
      const acknowledgement=await (expectedRevision===undefined?onMove(from,to):onMove(from,to,expectedRevision));
      if(expectedRevision!==undefined){if(acknowledgement?.queueRevision!==expectedRevision+1)throw new Error("Queue revision acknowledgement changed");expectedRevision=acknowledgement.queueRevision;}
    } } if(signal?.aborted)throw new Error("Queue interaction cancelled"); selectedRows=plan.selected;selectionAnchor=selectedRows[0]??-1;
      if (focusOrigin) {
        await tick();
        // Keep repeated arrow presses on the moved occurrence, without stealing focus.
        if (document.activeElement === focusOrigin || document.activeElement === document.body) {
          const selector = focusOrigin.classList.contains("track-select") ? ".track-select" : ".queue-drag-select";
          const firstSelected = plan.selected[0];
          if (firstSelected !== undefined) panel?.querySelectorAll<HTMLElement>(`.queue-list article ${selector}`)[firstSelected]?.focus();
        }
      }
    }
    catch { moveFailureSerial++;selectedRows=[];selectionAnchor=-1;moveError="Could not finish moving queue tracks."; }
    finally { movingRows=false; }
  }
  function moveSelectionKey(event: KeyboardEvent, index: number) {
    if(!["ArrowUp","ArrowDown"].includes(event.key)||event.altKey||event.ctrlKey||event.metaKey||event.shiftKey)return;
    if((event.target as HTMLElement).closest("select,input,details"))return;
    event.preventDefault();event.stopPropagation();
    const rows=selectedRows.includes(index)?selectedRows:[index];
    const first=Math.min(...rows),last=Math.max(...rows);
    if(event.key==="ArrowUp"&&first>0)void moveSelection(rows,first-1,event.currentTarget as HTMLElement,!event.isTrusted);
    if(event.key==="ArrowDown"&&last<tracks.length-1)void moveSelection(rows,first+1,event.currentTarget as HTMLElement,!event.isTrusted);
  }
  let expanded: Record<string, boolean> = $state({});
  let sortField = $state<TrackSortField>("title");
  let sortDirection = $state<SortDirection>("asc");
  const upcomingTracks = $derived(tracks.slice(currentIndex + 1));
  const sortAvailability = $derived(trackSortAvailability(upcomingTracks));
  const currentChapter = $derived.by(() => {
    const track = tracks[currentIndex] as RangedTrack | undefined;
    const absolutePosition = chapterPosition ?? (track?.playbackRange
      ? track.playbackRange.startTime + chapterRelativePosition(track, position)
      : position);
    return activeChapter(chapters, absolutePosition);
  });
  function samePlaybackRange(left: PlaybackRange | undefined, right: PlaybackRange | undefined): boolean {
    return Boolean(left && right && left.parentTrackId === right.parentTrackId && left.chapterId === right.chapterId
      && left.sourceUrl === right.sourceUrl && left.startTime === right.startTime && left.endTime === right.endTime);
  }
  let radioHistoryExpanded = $state(false);
  let openMoreMenu: HTMLDetailsElement | null = null;
  function closeOpenMenu(returnFocus = false) {
    const active = openMoreMenu?.open ? openMoreMenu : document.querySelector<HTMLDetailsElement>("details.queue-more[open]");
    if (active) {
      active.open = false;
      if (returnFocus) active.querySelector<HTMLElement>("summary")?.focus();
    }
    openMoreMenu = null;
  }
  function dismissMenuOutside(event: PointerEvent) {
    const active = openMoreMenu?.open ? openMoreMenu : document.querySelector<HTMLDetailsElement>("details.queue-more[open]");
    if (active && event.target instanceof Node && !active.contains(event.target)) closeOpenMenu();
  }
  function dismissMenuEscape(event: KeyboardEvent) {
    if (event.key === "Escape" && (openMoreMenu?.open || document.querySelector("details.queue-more[open]"))) {
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
  function positionMoreMenu(event: Event) {
    const details = event.currentTarget as HTMLDetailsElement;
    const menu = details.querySelector<HTMLElement>(".queue-more-menu");
    if (!menu) return;
    if (!details.open) {
      menu.style.removeProperty("left");
      menu.style.removeProperty("top");
      if (openMoreMenu === details) openMoreMenu = null;
      return;
    }
    document.querySelectorAll<HTMLDetailsElement>("details.queue-more[open]").forEach((other) => {
      if (other !== details) other.open = false;
    });
    openMoreMenu = details;
    const anchor = details.querySelector("summary")?.getBoundingClientRect();
    if (!anchor) return;
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
  function runMenuAction(event: MouseEvent, action: () => unknown) {
    void action();
    const details = (event.currentTarget as HTMLElement).closest("details");
    if (openMoreMenu === details) openMoreMenu = null;
    details?.removeAttribute("open");
  }
  function toggleChapters(track: Track) {
    expanded = { ...expanded, [track.id]: !expanded[track.id] };
  }
  const time = (seconds: number) => {
    const value = Math.max(0, Math.floor(seconds));
    return Math.floor(value / 60) + ":" + String(value % 60).padStart(2, "0");
  };
</script>

<aside bind:this={panel} aria-busy={movingRows} data-queue-move-failures={moveFailureSerial} class="playback-queue-panel" aria-label="Playback queue">
  <header class="queue-header">
    <div><p class="eyebrow">PLAYER</p><h2>Queue <span>{tracks.length}</span></h2></div>
    {#if onIdentifyCurrent}<button class="identify" aria-label={identifyingCurrentAudio ? "Cancel current audio capture" : "Identify currently playing audio"} disabled={identifyingCurrentAudio && !onCancelIdentifyCurrent} onclick={identifyingCurrentAudio ? onCancelIdentifyCurrent ?? onIdentifyCurrent : onIdentifyCurrent}>{identifyingCurrentAudio ? onCancelIdentifyCurrent ? `Cancel capture · ${Math.floor(radioIdentifyProgress)} / 8 s` : "Identifying…" : "Identify audio"}</button>{/if}
    <button class="close" aria-label="Close playback queue" onclick={onClose}>✕</button>
  </header>

  <div class="queue-content">
    {#if tracks.length}
      <section class="queue-list" aria-label="Queued tracks">
        {#if moveError}<p role="alert">{moveError}</p>{/if}
        {#if onSortUpcoming && upcomingTracks.length > 1}
          <div class="queue-sort-upcoming">
            <label>Sort upcoming
              <select aria-label="Sort upcoming by" bind:value={sortField}>
                <option value="title" disabled={!sortAvailability.title}>{trackSortOptionLabel(sortAvailability, "title", "Title")}</option>
                <option value="artist" disabled={!sortAvailability.artist}>{trackSortOptionLabel(sortAvailability, "artist", "Artist")}</option>
                <option value="album" disabled={!sortAvailability.album}>{trackSortOptionLabel(sortAvailability, "album", "Album")}</option>
                <option value="duration" disabled={!sortAvailability.duration}>{trackSortOptionLabel(sortAvailability, "duration", "Duration")}</option>
                <option value="releaseDate" disabled={!sortAvailability.releaseDate}>{trackSortOptionLabel(sortAvailability, "releaseDate", "Release date")}</option>
                <option value="dateAdded" disabled={!sortAvailability.dateAdded}>{trackSortOptionLabel(sortAvailability, "dateAdded", "Date added")}</option>
                <option value="bpm" disabled={!sortAvailability.bpm}>{trackSortOptionLabel(sortAvailability, "bpm", "BPM")}</option>
                <option value="key" disabled={!sortAvailability.key}>{trackSortOptionLabel(sortAvailability, "key", "Key")}</option>
                <option value="plays" disabled={!sortAvailability.plays}>{trackSortOptionLabel(sortAvailability, "plays", "Plays")}</option>
                <option value="views" disabled={!sortAvailability.views}>{trackSortOptionLabel(sortAvailability, "views", "Views")}</option>
              </select>
            </label>
            <select aria-label="Sort direction" bind:value={sortDirection}>
              <option value="asc">Ascending</option>
              <option value="desc">Descending</option>
            </select>
            <button type="button" onclick={() => onSortUpcoming?.(sortField, sortDirection)}>Sort</button>
          </div>
        {/if}
        {#each tracks as track, index (track.id + ":" + index)}
          {@const trackChapters = chaptersByTrack[track.id] ?? []}
          {@const duration=playableDuration(track)}
          <article class:queue-selected={selectedRows.includes(index)} ondragover={event=>{if(draggedRows.length&&!movingRows)event.preventDefault();}} ondrop={event=>{event.preventDefault();if(draggedRows.length){const rows=draggedRows;draggedRows=[];void moveSelection(rows,index-rows.filter(at=>at<index).length,undefined,!event.isTrusted,draggedRevision);}}} class:current={index === currentIndex} class:paused={index === currentIndex && paused} class:buffering={index === currentIndex && buffering} aria-current={index === currentIndex ? "true" : undefined} aria-busy={index === currentIndex && buffering ? "true" : undefined}>
            <button onkeydown={event=>moveSelectionKey(event,index)} class="queue-drag-select" draggable={!movingRows} aria-label={`Select queue occurrence ${index+1}: ${track.title}`} aria-pressed={selectedRows.includes(index)} title="Drag to reorder; Ctrl or Shift click to select; arrow keys move selection" onclick={event=>selectQueueRow(index,event)} ondragstart={event=>{if(movingRows){event.preventDefault();return;}draggedRevision=queueRevision;draggedRows=selectedRows.includes(index)?[...selectedRows]:[index];selectedRows=draggedRows;event.dataTransfer?.setData("text/plain","burrata-queue-row");if(event.dataTransfer)event.dataTransfer.effectAllowed="move";}} ondragend={()=>draggedRows=[]}>⠿</button>
            <button onkeydown={event=>moveSelectionKey(event,index)} class="track-select" title={`${track.title}${track.artist ? ` · ${track.artist}` : ""}`} aria-label={index === currentIndex ? `${buffering ? "Buffering" : paused ? "Paused" : "Playing"} ${track.title}` : "Play " + track.title} onclick={event => { if(event.ctrlKey||event.metaKey||event.shiftKey)selectQueueRow(index,event);else onPlay(index); }}>
              <span class="track-index">{index === currentIndex ? buffering ? "…" : paused ? "Ⅱ" : "♫" : String(index + 1).padStart(2, "0")}</span>
              <span class="track-copy"><strong>{track.title}{track.version ? " (" + track.version + ")" : ""}</strong><small>{track.artist || "Unknown artist"}{track.album ? " · " + track.album : ""}</small></span>
              {#if duration!==null}<span class="track-duration">{time(duration)}</span>{/if}
            </button>
            <details class="queue-more" ontoggle={positionMoreMenu}><summary aria-label={"More actions for " + track.title} title="More actions">⋯</summary><div class="queue-more-menu">
              {#if onAddToPlaylist}<button class="primary-action" onclick={(event) => runMenuAction(event, () => onAddToPlaylist?.(track, index))}><MenuIcon name="plus"/><span>Add to playlist</span></button>{/if}
              <button class="menu-section" disabled={index === 0} onclick={(event) => runMenuAction(event, () => moveSelection([index], index - 1))}><MenuIcon name="arrow-up"/><span>Move up</span></button>
              <button disabled={index === tracks.length - 1} onclick={(event) => runMenuAction(event, () => moveSelection([index], index + 1))}><MenuIcon name="arrow-down"/><span>Move down</span></button>
              {#if onInfoTrack}<button class="menu-section" onclick={(event) => runMenuAction(event, () => onInfoTrack?.(track))}><MenuIcon name="info"/><span>Track information</span></button>{/if}
              <button class="menu-section" disabled={index === currentIndex} title={index === currentIndex ? "Current track" : "Remove from queue"} onclick={(event) => runMenuAction(event, () => onRemove(index))}><MenuIcon name="remove"/><span>Remove from queue</span></button>
              {#if trackChapters.length}<button class="menu-section" aria-expanded={!!expanded[track.id]} onclick={(event) => runMenuAction(event, () => toggleChapters(track))}><MenuIcon name="queue"/><span>{expanded[track.id] ? "Hide chapters" : "Show chapters"}</span></button>
                {#if onAppendChapters}<button onclick={(event) => runMenuAction(event, () => onAppendChapters?.(track, trackChapters))}><MenuIcon name="queue"/><span>Append chapters</span></button>{/if}
                {#if onQueueAllChapters}<button onclick={(event) => runMenuAction(event, () => onQueueAllChapters?.(track, trackChapters))}><MenuIcon name="queue"/><span>Queue all chapters</span></button>{/if}
              {:else if onInspectChapters && !track.playbackRange && ["youtube","soundcloud"].includes(track.provider)}<button class="menu-section" onclick={(event) => runMenuAction(event, () => onInspectChapters?.(track))}><MenuIcon name="search"/><span>Check for chapters</span></button>{/if}
            </div></details>
          </article>
          {#if trackChapters.length && expanded[track.id]}
            <div class="chapter-children" aria-label={"Chapters for " + track.title}>
              {#each trackChapters as chapter (chapter.id)}
                {@const range = chapterRangesByTrack[track.id]?.[chapter.id]}
                {@const chapterCurrent = index === currentIndex && samePlaybackRange(activePlaybackTrack?.playbackRange, range)}
                {@const chapterBuffering = chapterCurrent && buffering}
                <div class="chapter-child" class:current={chapterCurrent} class:paused={chapterCurrent && paused} class:buffering={chapterBuffering} aria-current={chapterCurrent ? "true" : undefined} aria-busy={chapterBuffering ? "true" : undefined}>
                  <button class="chapter-child-play" class:active={chapterCurrent} aria-label={"Play chapter " + chapter.title} onclick={() => onPlayChapter?.(track, chapter, index)}>
                    <span>{time(chapter.startTime)}</span><strong>{chapter.title}</strong><span>{time(chapter.endTime - chapter.startTime)}</span>
                  </button>
                  <details class="queue-more chapter-more" ontoggle={positionMoreMenu}><summary aria-label={"More actions for chapter " + chapter.title} title="Chapter actions">⋯</summary><div class="queue-more-menu">
                    {#if onAddChapterToPlaylist}<button class="primary-action" onclick={(event) => runMenuAction(event, () => onAddChapterToPlaylist?.(track, chapter, index))}><MenuIcon name="plus"/><span>Add chapter to playlist</span></button>{/if}
                    {#if onQueueChapter}<button class="menu-section" onclick={(event) => runMenuAction(event, () => onQueueChapter?.(track, chapter, index))}><MenuIcon name="queue"/><span>Queue chapter</span></button>{/if}
                    {#if onDownloadChapter}<button class="menu-section" onclick={(event) => runMenuAction(event, () => onDownloadChapter?.(track, chapter, index))}><MenuIcon name="download"/><span>Download chapter</span></button>{/if}
                    {#if onInfoChapter}<button class="menu-section" onclick={(event) => runMenuAction(event, () => onInfoChapter?.(track, chapter, index))}><MenuIcon name="search"/><span>Search for more info</span></button>{/if}
                  </div></details>
                </div>
              {/each}
            </div>
          {/if}
        {/each}
      </section>
    {:else}
      <div class="queue-empty"><span aria-hidden="true">♫</span><strong>Your queue is empty</strong><p>Choose a track to start listening.</p></div>
    {/if}

    {#if radioNowPlaying || onIdentifyRadio}
      <section class="radio-current" aria-label="Radio currently playing">
        <header><div><p class="eyebrow">LIVE RADIO</p><h3>{radioStationName || "Current station"}</h3></div>
          {#if onIdentifyRadio}<button class="identify" disabled={radioRecognizing && !onCancelIdentifyRadio} onclick={radioRecognizing ? onCancelIdentifyRadio ?? onIdentifyRadio : onIdentifyRadio}>{radioRecognizing ? onCancelIdentifyRadio ? `Cancel capture · ${Math.floor(radioIdentifyProgress)} / 8 s` : "Identifying…" : "Identify song"}</button>{/if}
        </header>
        {#if radioNowPlaying}<div class="radio-now-row"><div class="radio-track"><strong>{radioNowPlaying.title}</strong><small>{radioNowPlaying.artist || "Unknown artist"}{radioNowPlaying.album ? " · " + radioNowPlaying.album : ""}</small></div>
        {#if onContextRadioTrack}<button class="radio-more" aria-label={"Actions for " + radioNowPlaying.title} onclick={()=>onContextRadioTrack?.(radioNowPlaying)}>⋯</button>{/if}</div>
        {:else}<p class="radio-track">No song identified yet.</p>{/if}
      </section>
    {/if}

    {#if radioHistory.length}
      <section class="radio-history" aria-label="Previously played on radio">
        <button class="history-toggle" aria-expanded={radioHistoryExpanded} onclick={() => radioHistoryExpanded = !radioHistoryExpanded}>
          <span>Previously played on radio</span><span>{radioHistory.length} {radioHistoryExpanded ? "⌃" : "⌄"}</span>
        </button>
        {#if radioHistoryExpanded}
          <div class="history-list">
            {#each radioHistory as occurrence (occurrence.id)}
              <article class="history-row">
                <button class="history-play" aria-label={"Play " + occurrence.track.title} onclick={() => onPlayRadioTrack?.(occurrence.track)}>
                  <strong>{occurrence.track.title}</strong><small>{occurrence.track.artist || "Unknown artist"} · {occurrence.stationName}</small>
                  <time datetime={occurrence.playedAt}>{new Date(occurrence.playedAt).toLocaleString()}</time>
                </button>
                {#if onContextRadioTrack}<button class="radio-more" aria-label={"Actions for " + occurrence.track.title} onclick={() => onContextRadioTrack?.(occurrence.track)}>⋯</button>{/if}
              </article>
            {/each}
          </div>
        {/if}
      </section>
    {/if}

    {#if chapters.length}
      <section class="chapter-list" aria-label="Chapters">
        <header><div><p class="eyebrow">CURRENT AUDIO</p><h3>Chapters</h3></div></header>
        {#each chapters as chapter (chapter.id)}
          <div class="chapter-row"><button class="chapter-play" class:active={currentChapter?.id === chapter.id} class:paused={currentChapter?.id === chapter.id && paused} class:buffering={currentChapter?.id === chapter.id && buffering} aria-label={currentChapter?.id === chapter.id ? `${buffering ? "Buffering" : paused ? "Paused" : "Playing"} chapter ${chapter.title}` : "Seek to chapter " + chapter.title} aria-current={currentChapter?.id === chapter.id ? "time" : undefined} aria-busy={currentChapter?.id === chapter.id && buffering ? "true" : undefined} onclick={() => onSeek(chapter.startTime)}>
            <span>{time(chapter.startTime)}</span><strong>{chapter.title}</strong><span class="chapter-play-icon" aria-hidden="true">{currentChapter?.id === chapter.id ? buffering ? "…" : paused ? "Ⅱ" : "♫" : "▶"}</span>
          </button><details class="queue-more chapter-more" ontoggle={positionMoreMenu}><summary aria-label={"More actions for chapter " + chapter.title} title="Chapter actions">⋯</summary><div class="queue-more-menu">
            {#if onAddChapterToPlaylist}<button class="primary-action" onclick={(event) => runMenuAction(event, () => onAddChapterToPlaylist?.(tracks[currentIndex]!, chapter, currentIndex))}><MenuIcon name="plus"/><span>Add chapter to playlist</span></button>{/if}
            {#if onQueueChapter}<button class="menu-section" onclick={(event) => runMenuAction(event, () => onQueueChapter?.(tracks[currentIndex]!, chapter, currentIndex))}><MenuIcon name="queue"/><span>Queue chapter</span></button>{/if}
            {#if onDownloadChapter}<button class="menu-section" onclick={(event) => runMenuAction(event, () => onDownloadChapter?.(tracks[currentIndex]!, chapter, currentIndex))}><MenuIcon name="download"/><span>Download chapter</span></button>{/if}
            {#if onInfoChapter}<button class="menu-section" onclick={(event) => runMenuAction(event, () => onInfoChapter?.(tracks[currentIndex]!, chapter, currentIndex))}><MenuIcon name="search"/><span>Search for more info</span></button>{/if}
          </div></details></div>
        {/each}
      </section>
    {/if}
  </div>
</aside>

<style>
  .playback-queue-panel {
    position: fixed;
    z-index: 40;
    top: 12px;
    right: 12px;
    bottom: calc(var(--player-height, 92px) + 12px + env(safe-area-inset-bottom));
    display: flex;
    flex-direction: column;
    width: min(440px, calc(100vw - 24px));
    min-height: 0;
    overflow: hidden;
    border: 1px solid var(--line);
    border-radius: 15px;
    background: var(--surface);
    color: var(--text);
    box-shadow: 0 18px 60px #0009;
  }

  .queue-sort-upcoming {
    display: flex;
    flex-wrap: wrap;
    align-items: end;
    gap: 6px;
    min-width: 0;
    padding: 8px 10px;
    border-bottom: 1px solid var(--line);
  }
  .queue-sort-upcoming label {
    display: grid;
    gap: 4px;
    flex: 1 1 130px;
    min-width: 0;
    margin: 0;
    color: var(--muted);
    font-size: 11px;
  }
  .queue-sort-upcoming label select { width:100%; }
  .queue-sort-upcoming > select { flex:0 1 108px; width:108px; }
  .queue-sort-upcoming select,
  .queue-sort-upcoming button {
    box-sizing: border-box;
    min-width: 0;
    max-width: 100%;
    min-height: 34px;
    height: auto;
    line-height: normal;
    border: 1px solid var(--line);
    border-radius: 8px;
    background: var(--surface-raised, var(--surface));
    color: var(--text);
    padding: 0 8px;
    font: inherit;
  }
  .queue-sort-upcoming > select { font-size:12px; }
  .queue-sort-upcoming button { flex:none; cursor:pointer; font-size:12px; }
  .queue-header { display: flex; flex: 0 0 auto; align-items: center; justify-content: space-between; gap: 14px; padding: 15px 17px 12px; border-bottom: 1px solid var(--line); }
  .eyebrow { margin: 0 0 4px; color: var(--muted); font-size: 9px; font-weight: 650; letter-spacing: .13em; }
  .queue-header h2,.chapter-list h3 { display: flex; align-items: center; gap: 8px; margin: 0; font-size: 16px; letter-spacing: -.02em; }
  .queue-header h2 span { display: grid; place-items: center; min-width: 20px; height: 20px; padding: 0 5px; border-radius: 10px; background: var(--raised); color: var(--muted); font-size: 10px; }
  .close { display: grid; place-items: center; width: 32px; height: 32px; padding: 0; border-radius: 8px; background: transparent; color: var(--muted); }
  .queue-content { min-height: 0; flex: 1; overflow-y: auto; overscroll-behavior: contain; }
  .queue-list { padding: 5px 9px 8px; }
  .queue-list article { display: flex; align-items: center; gap: 3px; min-width: 0; padding: 4px 3px; border-bottom: 1px solid color-mix(in srgb, var(--line) 66%, transparent); border-radius: 7px; }
  .queue-list article.current { background: color-mix(in srgb, var(--accent) 10%, transparent); }
  .queue-list article.paused .track-index,.queue-list article.buffering .track-index { color: var(--accent); }
  .chapter-child.current { border-radius: 6px; background: color-mix(in srgb, var(--accent) 10%, transparent); }
  .chapter-child.current .chapter-child-play strong { color: var(--accent); }
  .chapter-child.paused .chapter-child-play::after { content: "Ⅱ"; color: var(--accent); }
  .chapter-child.buffering .chapter-child-play::after { content: "…"; color: var(--accent); }
  .track-select { display: flex; align-items: center; gap: 10px; flex: 1; min-width: 0; padding: 6px; background: transparent; text-align: left; }
  .track-index { flex: 0 0 27px; color: var(--muted); font-size: 10px; font-variant-numeric: tabular-nums; text-align: center; }
  .current .track-index { color: var(--accent); }
  .track-copy { display: grid; flex: 1; min-width: 0; gap: 3px; }
  .track-copy strong,.track-copy small { overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .track-copy strong { display:-webkit-box;-webkit-box-orient:vertical;-webkit-line-clamp:2;line-clamp:2;white-space:normal;overflow-wrap:anywhere;line-height:1.35;color:var(--text);font-size:12px;font-weight:590; }
  .current .track-copy strong { color: var(--accent); }
  .track-copy small { color: var(--muted); font-size: 10px; }
  .track-duration { flex: 0 0 auto; padding-left: 4px; color: var(--muted); font-size: 9px; font-variant-numeric: tabular-nums; }
  .queue-more { position:relative; flex:0 0 auto; }
  .queue-more summary,.radio-more { display:grid;place-items:center;list-style:none;width:40px;height:40px;padding:0;border:1px solid transparent;border-radius:8px;background:transparent;color:var(--muted);font-size:23px;cursor:pointer;touch-action:manipulation; }
  .queue-more summary::-webkit-details-marker { display:none; }
  .queue-more summary:hover,.radio-more:hover { color:var(--accent);background:var(--raised); }
  .queue-more summary:focus-visible,.radio-more:focus-visible,.queue-more-menu button:focus-visible { outline:2px solid var(--accent);outline-offset:2px; }
  .queue-more-menu { position:fixed;z-index:90;top:8px;left:8px;right:auto;display:flex;flex-direction:column;box-sizing:border-box;width:max-content;min-width:176px;max-width:min(260px,calc(100vw - 16px));max-height:calc(100dvh - var(--player-height,90px) - 16px);overflow:auto;padding:5px;border:1px solid var(--line);border-radius:9px;background:var(--surface);box-shadow:0 10px 30px #0008; }
  .queue-more-menu button { display:flex;align-items:center;gap:10px;justify-content:flex-start;flex:0 0 auto;width:100%;min-width:0;min-height:36px;padding:5px 9px;border:0;border-radius:5px;background:transparent;color:var(--text);font:inherit;font-size:13px;line-height:1.35;text-align:left;white-space:normal;overflow-wrap:anywhere;touch-action:manipulation; }
  .queue-more-menu button span { min-width:0; }
  .queue-more-menu :global(svg[aria-hidden="true"]) { display:block;flex:0 0 20px;color:var(--muted); }
  .queue-more-menu button.menu-section { margin-top:5px;padding-top:7px;border-top:1px solid var(--line);border-top-left-radius:0;border-top-right-radius:0; }
  .queue-more-menu button.primary-action { margin:3px 0;background:color-mix(in srgb,var(--accent) 18%,var(--surface));font-weight:650; }
  .queue-more-menu button.primary-action:hover { background:color-mix(in srgb,var(--accent) 28%,var(--surface)); }
  .queue-more-menu button.primary-action :global(svg[aria-hidden="true"]) { color:var(--accent); }
  .queue-more-menu button:hover { background:var(--raised); }
  .queue-more-menu button:disabled { opacity:.45; }
  .chapter-children { margin: -2px 4px 6px 42px; padding: 4px 6px 6px; border-left: 1px solid var(--line); }
  .chapter-child { display: flex; align-items: center; gap: 3px; }
  .chapter-child-play { display: grid; flex: 1; grid-template-columns: 35px minmax(0,1fr) 35px; align-items: center; gap: 7px; padding: 6px 4px; background: transparent; color: var(--muted); text-align: left; }
  .chapter-child-play span { font-size: 9px; font-variant-numeric: tabular-nums; }
  .chapter-child-play strong { overflow: hidden; color: var(--text); font-size: 10px; font-weight: 500; text-overflow: ellipsis; white-space: nowrap; }
  .chapter-more summary { width:36px;height:36px; }
  .queue-empty { display: grid; justify-items: center; align-content: center; min-height: 180px; height: 100%; color: var(--muted); text-align: center; }
  .queue-empty span { display: grid; place-items: center; width: 42px; height: 42px; margin-bottom: 11px; border-radius: 13px; background: var(--raised); color: var(--accent); font-size: 22px; }
  .queue-empty strong { color: var(--text); font-size: 12px; }
  .queue-empty p { margin: 5px 0 0; font-size: 10px; }
  .radio-current,.radio-history { padding: 12px 13px; border-top: 1px solid var(--line); }
  .radio-current > header { display: flex; align-items: center; justify-content: space-between; gap: 8px; }
  .radio-current h3 { margin: 0; font-size: 12px; }
  .identify { min-height: 34px; padding: 6px 10px; border-radius: 7px; background: var(--accent); color: var(--accent-contrast, #111); font-size: 10px; font-weight: 650; }
  .identify:disabled { opacity: .6; }
  .radio-track { display: grid; min-width: 0; gap: 3px; margin-top: 8px; }
  .radio-track strong,.radio-track small { overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .radio-track strong { font-size: 12px; }
  .radio-track small { color: var(--muted); font-size: 10px; }
  .radio-now-row { display:flex;align-items:center;gap:4px;min-width:0; }
  .radio-now-row .radio-track { flex:1; }
  .radio-more { flex:0 0 40px; }
  .radio-history { padding-top: 7px; }
  .history-toggle { display: flex; align-items: center; justify-content: space-between; width: 100%; min-height: 36px; padding: 5px 2px; background: transparent; color: var(--text); text-align: left; font-size: 11px; font-weight: 600; }
  .history-toggle span:last-child { color: var(--muted); font-size: 10px; }
  .history-list { display: grid; gap: 3px; }
  .history-row { display: flex; align-items: center; min-width: 0; gap: 4px; padding: 4px 0; border-top: 1px solid color-mix(in srgb, var(--line) 66%, transparent); }
  .history-play { display: grid; flex: 1; min-width: 0; gap: 3px; padding: 5px 3px; background: transparent; text-align: left; }
  .history-play strong,.history-play small { overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .history-play strong { font-size: 11px; }
  .history-play small,.history-play time { color: var(--muted); font-size: 9px; }
  .history-row .history-play { flex:1; }
  .chapter-list { padding: 12px 13px 15px; border-top: 1px solid var(--line); }
  .chapter-list > header { margin-bottom: 7px; }
  .chapter-list h3 { font-size: 12px; }
  .chapter-row { display:flex;align-items:center;gap:2px;min-width:0; }
  .chapter-list .chapter-play { display: grid; flex:1;grid-template-columns:35px minmax(0,1fr) 16px;align-items:center;gap:8px;min-width:0;padding:8px 6px;border-radius:6px;background:transparent;color:var(--muted);text-align:left; }
  .chapter-list .chapter-play.active { background: color-mix(in srgb, var(--accent) 10%, transparent); color: var(--accent); }
  .chapter-list .chapter-play.active.paused,.chapter-list .chapter-play.active.buffering { outline: 1px solid color-mix(in srgb, var(--accent) 35%, transparent); }
  .chapter-list .chapter-play > span:first-child { font-size:9px;font-variant-numeric:tabular-nums; }
  .chapter-list .chapter-play strong { overflow:hidden;color:var(--text);font-size:10px;font-weight:530;text-overflow:ellipsis;white-space:nowrap; }
  .chapter-list .chapter-play.active strong { color:var(--accent); }
  .chapter-play-icon { visibility:hidden;font-size:9px; }
  .chapter-list .chapter-play:hover .chapter-play-icon,.chapter-list .chapter-play.active .chapter-play-icon { visibility:visible; }
  @media (max-width: 767px) {
    .playback-queue-panel { top: 8px; right: 8px; bottom: calc(var(--player-height, 100px) + 8px + env(safe-area-inset-bottom)); width: calc(100vw - 16px); border-radius: 14px; }
    .queue-header { padding: 12px 14px 10px; }
    .queue-list { padding-right: 6px; padding-left: 6px; }
  }
  @media (max-width: 360px) {
    .track-duration { display: none; }
    .queue-more summary,.chapter-more summary,.radio-more { width:36px;min-width:36px;height:36px; }
  }
  .queue-list article.queue-selected { outline:1px solid var(--accent); outline-offset:-1px; }
  .queue-drag-select { flex:0 0 24px; width:24px; min-width:24px; box-sizing:border-box; cursor:grab; padding:4px 2px; touch-action:none; }

  @media (min-width:601px) {
    .queue-more-menu { padding:5px;border-radius:6px;border-color:var(--context-menu-border,var(--line));background:var(--context-menu-surface,var(--surface));color:var(--context-menu-text,var(--text));box-shadow:0 8px 28px #0007; }
    .queue-more-menu button { min-height:32px;gap:10px;padding:4px 8px;border-radius:3px;color:var(--context-menu-text,var(--text));line-height:1.3; }
    .queue-more-menu button:hover { background:var(--context-menu-hover,var(--raised)); }
    .queue-more-menu button.primary-action { margin:1px 0;background:var(--context-menu-primary,var(--raised));font-weight:600; }
    .queue-more-menu button.primary-action:hover { background:var(--context-menu-hover,var(--raised)); }
    .queue-more-menu :global(svg[aria-hidden="true"]) { width:18px;height:18px;flex-basis:18px;color:var(--context-menu-muted,var(--muted)); }
    .queue-more-menu button.menu-section { margin-top:4px;padding-top:7px;border-top-color:var(--context-menu-border,var(--line)); }
  }
</style>
