<script lang="ts">
  import type { Chapter, Track } from "./lib/types";
  import { activeChapter, chapterRelativePosition } from "./lib/chapters";
  import type { RangedTrack } from "./lib/chapters";

  let {
    tracks,
    currentIndex,
    chapters = [],
    position,
    repeatMode,
    shuffleEnabled,
    onClose,
    onPlay,
    onMove,
    onRemove,
    onSeek,
    onToggleShuffle,
    onCycleRepeat,
    onInspectChapters,
    chaptersByTrack = {},
    onPlayChapter,
    onQueueChapter,
    onAppendChapters,
    onDownloadChapter,
    onInfoChapter,
    onQueueAllChapters,
  }: {
    tracks: Track[];
    currentIndex: number;
    chapters?: Chapter[];
    position: number;
    repeatMode: "off" | "all" | "one";
    shuffleEnabled: boolean;
    onClose: () => void;
    onPlay: (index: number) => void;
    onMove: (from: number, to: number) => void;
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
    onInfoChapter?: (parent: Track, chapter: Chapter, parentIndex: number) => void;
    onQueueAllChapters?: (parent: Track, chapters: Chapter[]) => void;
  } = $props();

  let expanded: Record<string, boolean> = $state({});
  const time = (seconds: number) => {
    const value = Math.max(0, Math.floor(seconds));
    return Math.floor(value / 60) + ":" + String(value % 60).padStart(2, "0");
  };
</script>

<aside class="playback-queue-panel" aria-label="Playback queue">
  <header class="queue-header">
    <div><p class="eyebrow">PLAYER</p><h2>Up next <span>{tracks.length}</span></h2></div>
    <button class="close" aria-label="Close playback queue" onclick={onClose}>✕</button>
  </header>

  <div class="queue-controls">
    <button aria-pressed={shuffleEnabled} class:active={shuffleEnabled} onclick={onToggleShuffle}>Shuffle {shuffleEnabled ? "on" : "off"}</button>
    <button onclick={onCycleRepeat}>Repeat {repeatMode}</button>
  </div>

  <div class="queue-content">
    {#if tracks.length}
      <section class="queue-list" aria-label="Queued tracks">
        {#each tracks as track, index (track.id + ":" + index)}
          {@const trackChapters = chaptersByTrack[track.id] ?? []}
          <article class:current={index === currentIndex} aria-current={index === currentIndex ? "true" : undefined}>
            <button class="track-select" aria-label={"Play " + track.title} onclick={() => onPlay(index)}>
              <span class="track-index">{index === currentIndex ? "♫" : String(index + 1).padStart(2, "0")}</span>
              <span class="track-copy"><strong>{track.title}{track.version ? " (" + track.version + ")" : ""}</strong><small>{track.artist || "Unknown artist"}{track.album ? " · " + track.album : ""}</small></span>
              {#if track.durationSeconds}<span class="track-duration">{time(track.durationSeconds)}</span>{/if}
            </button>
            <div class="row-controls">
              {#if trackChapters.length}<button aria-label={(expanded[track.id] ? "Collapse" : "Expand") + " chapters for " + track.title} aria-expanded={!!expanded[track.id]} onclick={() => expanded = { ...expanded, [track.id]: !expanded[track.id] }}>⌄</button>{:else if onInspectChapters && !track.playbackRange && ["youtube","soundcloud"].includes(track.provider)}<button aria-label={"Check chapters for " + track.title} onclick={()=>onInspectChapters?.(track)}>⌄</button>{/if}
              <button aria-label={"Move " + track.title + " up"} title="Move up" disabled={index === 0} onclick={() => onMove(index, index - 1)}>↑</button>
              <button aria-label={"Move " + track.title + " down"} title="Move down" disabled={index === tracks.length - 1} onclick={() => onMove(index, index + 1)}>↓</button>
              <button class="remove" aria-label={"Remove " + track.title + " from queue"} disabled={index === currentIndex} title={index === currentIndex ? "Current track" : "Remove from queue"} onclick={() => onRemove(index)}>×</button>
            </div>
          </article>
          {#if trackChapters.length && expanded[track.id]}
            <div class="chapter-children" aria-label={"Chapters for " + track.title}>
              <div class="chapter-actions">
                {#if onAppendChapters}<button onclick={() => onAppendChapters?.(track, trackChapters)}>Append chapters</button>{/if}
                {#if onQueueAllChapters}<button onclick={() => onQueueAllChapters?.(track, trackChapters)}>Queue all</button>{/if}
              </div>
              {#each trackChapters as chapter (chapter.id)}
                <div class="chapter-child">
                  <button class="chapter-child-play" aria-label={"Play chapter " + chapter.title} onclick={() => onPlayChapter?.(track, chapter, index)}>
                    <span>{time(chapter.startTime)}</span><strong>{chapter.title}</strong><span>{time(chapter.endTime - chapter.startTime)}</span>
                  </button>
                  <div class="chapter-child-actions">
                    {#if onQueueChapter}<button aria-label={"Queue chapter " + chapter.title} onclick={() => onQueueChapter?.(track, chapter, index)}>＋</button>{/if}
                    {#if onDownloadChapter}<button aria-label={"Download chapter " + chapter.title} onclick={() => onDownloadChapter?.(track, chapter, index)}>⇩</button>{/if}
                    {#if onInfoChapter}<button aria-label={"Search for more info about " + chapter.title} onclick={() => onInfoChapter?.(track, chapter, index)}>ⓘ</button>{/if}
                  </div>
                </div>
              {/each}
            </div>
          {/if}
        {/each}
      </section>
    {:else}
      <div class="queue-empty"><span aria-hidden="true">♫</span><strong>Your queue is empty</strong><p>Choose a track to start listening.</p></div>
    {/if}

    {#if chapters.length}
      <section class="chapter-list" aria-label="Chapters">
        <header><div><p class="eyebrow">CURRENT AUDIO</p><h3>Chapters</h3></div></header>
        {#each chapters as chapter (chapter.id)}
          {@const selectedChapter = activeChapter(chapters, (tracks[currentIndex] as RangedTrack)?.playbackRange ? (tracks[currentIndex] as RangedTrack).playbackRange!.startTime + chapterRelativePosition(tracks[currentIndex]!, position) : position)}
          <button class:active={selectedChapter?.id === chapter.id} aria-current={selectedChapter?.id === chapter.id ? "time" : undefined} onclick={() => onSeek(chapter.startTime)}>
            <span>{time(chapter.startTime)}</span><strong>{chapter.title}</strong><span class="chapter-play" aria-hidden="true">▶</span>
          </button>
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
  .queue-header { display: flex; flex: 0 0 auto; align-items: center; justify-content: space-between; gap: 14px; padding: 15px 17px 12px; border-bottom: 1px solid var(--line); }
  .eyebrow { margin: 0 0 4px; color: var(--muted); font-size: 9px; font-weight: 650; letter-spacing: .13em; }
  .queue-header h2,.chapter-list h3 { display: flex; align-items: center; gap: 8px; margin: 0; font-size: 16px; letter-spacing: -.02em; }
  .queue-header h2 span { display: grid; place-items: center; min-width: 20px; height: 20px; padding: 0 5px; border-radius: 10px; background: var(--raised); color: var(--muted); font-size: 10px; }
  .close { display: grid; place-items: center; width: 32px; height: 32px; padding: 0; border-radius: 8px; background: transparent; color: var(--muted); }
  .queue-controls { display: flex; flex: 0 0 auto; gap: 7px; padding: 10px 13px; border-bottom: 1px solid var(--line); }
  .queue-controls button { padding: 6px 10px; border-radius: 7px; color: var(--muted); font-size: 10px; }
  .queue-controls button.active { color: var(--accent); }
  .queue-content { min-height: 0; flex: 1; overflow-y: auto; overscroll-behavior: contain; }
  .queue-list { padding: 5px 9px 8px; }
  .queue-list article { display: flex; align-items: center; gap: 3px; min-width: 0; padding: 4px 3px; border-bottom: 1px solid color-mix(in srgb, var(--line) 66%, transparent); border-radius: 7px; }
  .queue-list article.current { background: color-mix(in srgb, var(--accent) 10%, transparent); }
  .track-select { display: flex; align-items: center; gap: 10px; flex: 1; min-width: 0; padding: 6px; background: transparent; text-align: left; }
  .track-index { flex: 0 0 27px; color: var(--muted); font-size: 10px; font-variant-numeric: tabular-nums; text-align: center; }
  .current .track-index { color: var(--accent); }
  .track-copy { display: grid; flex: 1; min-width: 0; gap: 3px; }
  .track-copy strong,.track-copy small { overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .track-copy strong { color: var(--text); font-size: 11px; font-weight: 590; }
  .current .track-copy strong { color: var(--accent); }
  .track-copy small { color: var(--muted); font-size: 10px; }
  .track-duration { flex: 0 0 auto; padding-left: 4px; color: var(--muted); font-size: 9px; font-variant-numeric: tabular-nums; }
  .row-controls { display: flex; flex: 0 0 auto; }
  .row-controls button { display: grid; place-items: center; width: 27px; height: 27px; padding: 0; border-radius: 6px; background: transparent; color: var(--muted); font-size: 12px; }
  .row-controls button:disabled { opacity: .3; }
  .row-controls .remove { font-size: 16px; }
  .chapter-children { margin: -2px 4px 6px 42px; padding: 4px 6px 6px; border-left: 1px solid var(--line); }
  .chapter-actions { display: flex; gap: 5px; margin: 0 0 3px 4px; }
  .chapter-actions button { padding: 4px 7px; border-radius: 5px; background: var(--raised); color: var(--muted); font-size: 9px; }
  .chapter-child { display: flex; align-items: center; gap: 3px; }
  .chapter-child-play { display: grid; flex: 1; grid-template-columns: 35px minmax(0,1fr) 35px; align-items: center; gap: 7px; padding: 6px 4px; background: transparent; color: var(--muted); text-align: left; }
  .chapter-child-play span { font-size: 9px; font-variant-numeric: tabular-nums; }
  .chapter-child-play strong { overflow: hidden; color: var(--text); font-size: 10px; font-weight: 500; text-overflow: ellipsis; white-space: nowrap; }
  .chapter-child-actions { display: flex; }
  .chapter-child-actions button { width: 26px; height: 26px; padding: 0; border-radius: 5px; background: transparent; color: var(--muted); }
  .queue-empty { display: grid; justify-items: center; align-content: center; min-height: 180px; height: 100%; color: var(--muted); text-align: center; }
  .queue-empty span { display: grid; place-items: center; width: 42px; height: 42px; margin-bottom: 11px; border-radius: 13px; background: var(--raised); color: var(--accent); font-size: 22px; }
  .queue-empty strong { color: var(--text); font-size: 12px; }
  .queue-empty p { margin: 5px 0 0; font-size: 10px; }
  .chapter-list { padding: 12px 13px 15px; border-top: 1px solid var(--line); }
  .chapter-list > header { margin-bottom: 7px; }
  .chapter-list h3 { font-size: 12px; }
  .chapter-list > button { display: grid; grid-template-columns: 35px minmax(0,1fr) 16px; align-items: center; gap: 8px; width: 100%; padding: 8px 6px; border-radius: 6px; background: transparent; color: var(--muted); text-align: left; }
  .chapter-list > button.active { background: color-mix(in srgb, var(--accent) 10%, transparent); color: var(--accent); }
  .chapter-list > button > span:first-child { font-size: 9px; font-variant-numeric: tabular-nums; }
  .chapter-list > button strong { overflow: hidden; color: var(--text); font-size: 10px; font-weight: 530; text-overflow: ellipsis; white-space: nowrap; }
  .chapter-list > button.active strong { color: var(--accent); }
  .chapter-play { visibility: hidden; font-size: 9px; }
  .chapter-list > button:hover .chapter-play,.chapter-list > button.active .chapter-play { visibility: visible; }
  @media (max-width: 767px) {
    .playback-queue-panel { top: 8px; right: 8px; bottom: calc(var(--player-height, 100px) + 8px + env(safe-area-inset-bottom)); width: calc(100vw - 16px); border-radius: 14px; }
    .queue-header { padding: 12px 14px 10px; }
    .queue-list { padding-right: 6px; padding-left: 6px; }
  }
</style>
