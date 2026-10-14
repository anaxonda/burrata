<script lang="ts">
  import { onMount } from "svelte";
  import type { Track } from "./lib/types";

  let {
    tracks, onPlay, onDownload, onFavorite, onNext, onQueue, onPlaylist, onArtist, onInfo,
    onContext, onSelect, selectedTrackIds = [], position, meta, timestamp, playingTrackId = "", bufferingTrackId = "", paused = false, listLabel = "Tracks",
  }: {
    tracks: Track[];
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
    selectedTrackIds?: string[];
    position?: (track: Track, index: number) => string | number | null | undefined;
    meta?: (track: Track, index: number) => string | null | undefined;
    timestamp?: (track: Track, index: number) => { label: string; full?: string } | null | undefined;
    playingTrackId?: string;
    bufferingTrackId?: string;
    paused?: boolean;
    listLabel?: string;
  } = $props();
  let openMoreMenu: HTMLDetailsElement | null = null;

  function closeOpenMenu() {
    const active = openMoreMenu?.open ? openMoreMenu : document.querySelector<HTMLDetailsElement>("details.more[open]");
    if (active) active.open = false;
    openMoreMenu = null;
  }
  function dismissMenuOutside(event: PointerEvent) {
    const active = openMoreMenu?.open ? openMoreMenu : document.querySelector<HTMLDetailsElement>("details.more[open]");
    if (active && event.target instanceof Node && !active.contains(event.target)) closeOpenMenu();
  }
  function dismissMenuEscape(event: KeyboardEvent) {
    if (event.key === "Escape" && (openMoreMenu?.open || document.querySelector("details.more[open]"))) closeOpenMenu();
  }
  function dismissMenuOnScroll(event: Event) {
    if (openMoreMenu && event.target instanceof Node && openMoreMenu.contains(event.target)) return;
    closeOpenMenu();
  }
  onMount(() => {
    window.addEventListener("pointerdown", dismissMenuOutside);
    window.addEventListener("keydown", dismissMenuEscape);
    window.addEventListener("scroll", dismissMenuOnScroll, true);
    window.addEventListener("resize", closeOpenMenu);
    return () => {
      window.removeEventListener("pointerdown", dismissMenuOutside);
      window.removeEventListener("keydown", dismissMenuEscape);
      window.removeEventListener("scroll", dismissMenuOnScroll, true);
      window.removeEventListener("resize", closeOpenMenu);
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
    const gutter = 8;
    const width = Math.min(box.width || menu.scrollWidth || 200, window.innerWidth - gutter * 2);
    const height = Math.min(menu.scrollHeight || box.height || 240, window.innerHeight - gutter * 2);
    const left = Math.max(gutter, Math.min(anchor.right - width, window.innerWidth - width - gutter));
    const roomBelow = window.innerHeight - anchor.bottom - gutter;
    const roomAbove = anchor.top - gutter;
    const top = height <= roomBelow || roomBelow >= roomAbove
      ? Math.max(gutter, Math.min(anchor.bottom + gutter, window.innerHeight - height - gutter))
      : Math.max(gutter, anchor.top - height - gutter);
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
</script>

<ol class="shared-track-list" aria-label={listLabel}>
  {#each tracks as track, index (`${track.id}:${index}`)}
    <li class:current={track.id === playingTrackId} class:buffering={track.id === bufferingTrackId} class:paused={track.id === playingTrackId && paused} class:selected={selectedTrackIds.includes(track.id)} aria-current={track.id === playingTrackId ? "true" : undefined} oncontextmenu={(event) => context(track, event)}>
      {#if onSelect}<input class="select-track" type="checkbox" checked={selectedTrackIds.includes(track.id)} aria-label={`Select ${track.title}`} onclick={(event) => { event.stopPropagation(); onSelect?.(track, event); }} />{/if}
      {#if position}<span class="track-position">{position(track, index) ?? index + 1}</span>{/if}
      {#if track.artworkUrl}<img class="artwork" src={track.artworkUrl} alt="" loading="lazy" />{:else}<span class="artwork placeholder" aria-hidden="true">♫</span>{/if}
      <div class="track-copy">
        <button class="track-title" aria-label={`Play ${track.title}`} onclick={() => onPlay?.(track, tracks, index)}>
          <strong>{track.title}{#if track.version}<small> ({track.version})</small>{/if}</strong>
        </button>
        <div class="track-subtitle">
          {#if onArtist}<button class="artist-link" onclick={() => onArtist?.(track)}>{track.artist}</button>{:else}<span>{track.artist}</span>{/if}
          {#if track.album}<span class="album"> · {track.album}</span>{/if}
          {#if meta?.(track, index)}<span> · {meta(track, index)}</span>{/if}
        </div>
        {#if timestamp?.(track, index)}<time class="track-time" title={timestamp(track, index)?.full}>{timestamp(track, index)?.label}</time>{/if}
      </div>
      {#if track.durationSeconds != null}<span class="duration">{Math.floor(track.durationSeconds / 60)}:{String(Math.floor(track.durationSeconds % 60)).padStart(2, "0")}</span>{/if}
      <div class="row-actions">
        {#if onPlay}<button class="icon-action play" aria-label={`Play ${track.title}`} title="Play" onclick={() => onPlay?.(track, tracks, index)}>▶</button>{/if}
        {#if onFavorite}<button class="icon-action favorite" class:active={track.favorite} aria-label={`${track.favorite ? "Remove" : "Add"} ${track.title} ${track.favorite ? "from" : "to"} favorites`} aria-pressed={track.favorite} title={track.favorite ? "Remove from favorites" : "Add to favorites"} onclick={() => onFavorite?.(track)}>{track.favorite ? "♥" : "♡"}</button>{/if}
        {#if onDownload}<button class="icon-action" aria-label={`Download ${track.title}`} title="Download" onclick={() => onDownload?.(track)}>↓</button>{/if}
        {#if onContext || onNext || onQueue || onPlaylist || onArtist || onInfo || track.sourceUrl}
          <details class="more" ontoggle={positionMoreMenu}><summary aria-label={`More actions for ${track.title}`} title="More actions">⋯</summary><div class="more-menu">
            {#if onNext}<button onclick={(event) => runMenuAction(event, () => onNext?.(track))}>Play next</button>{/if}
            {#if onQueue}<button onclick={(event) => runMenuAction(event, () => onQueue?.(track))}>Add to queue</button>{/if}
            {#if onPlaylist}<button onclick={(event) => runMenuAction(event, () => onPlaylist?.(track))}>Add to playlist</button>{/if}
            {#if onArtist}<button onclick={(event) => runMenuAction(event, () => onArtist?.(track))}>Open artist</button>{/if}
            {#if onInfo}<button onclick={(event) => runMenuAction(event, () => onInfo?.(track))}>Track information</button>{/if}
            {#if track.sourceUrl}<a href={track.sourceUrl} target="_blank" rel="noreferrer">Open source ↗</a>{/if}
            {#if onContext}<button onclick={(event) => runMenuAction(event, () => onContext?.(track, event))}>More actions…</button>{/if}
          </div></details>
        {/if}
      </div>
    </li>
  {/each}
</ol>

<style>
  .shared-track-list{list-style:none;margin:0;padding:0;min-width:0}.shared-track-list li{display:flex;align-items:center;gap:12px;min-width:0;padding:8px 4px;border-bottom:1px solid var(--line);transition:background 120ms ease}.shared-track-list li:hover{background:color-mix(in srgb,var(--raised) 72%,transparent)}.shared-track-list li.current{background:color-mix(in srgb,var(--accent) 12%,transparent)}.shared-track-list li.buffering .icon-action.play{animation:track-pulse 1s ease-in-out infinite alternate}.shared-track-list li.paused .icon-action.play{animation:none}@keyframes track-pulse{to{opacity:.45;transform:scale(.88)}}.track-position{flex:0 0 24px;text-align:right;color:var(--muted);font-size:12px}.artwork{flex:0 0 42px;width:42px;height:42px;object-fit:cover;border-radius:5px;background:var(--raised)}.placeholder{display:grid;place-items:center;color:var(--muted)}.track-copy{display:flex;flex:1;flex-direction:column;gap:3px;min-width:0}.track-title{display:block;max-width:100%;min-width:0;padding:0;border:0;background:transparent;color:var(--text);text-align:left}.track-title strong{display:block;overflow:hidden;text-overflow:ellipsis;white-space:nowrap;font-size:13px;font-weight:600}.track-title small{color:var(--muted);font-weight:400}.track-subtitle{overflow:hidden;text-overflow:ellipsis;white-space:nowrap;color:var(--muted);font-size:11px}.artist-link{padding:0;border:0;background:transparent;color:inherit;text-decoration:underline;text-underline-offset:2px}.track-time{color:var(--muted);font-size:10px}.duration{flex:0 0 38px;text-align:right;color:var(--muted);font-size:11px}.row-actions{display:flex;flex:0 0 auto;align-items:center;gap:4px}.icon-action,.more summary{display:grid;place-items:center;width:32px;height:32px;padding:0;border:1px solid var(--line);border-radius:50%;background:transparent;color:var(--muted);font-size:14px;cursor:pointer}.icon-action:hover,.more summary:hover{color:var(--accent);border-color:var(--accent)}.icon-action.play{color:var(--accent)}.icon-action.favorite.active{color:#f1768d}.more{position:relative}.more summary{list-style:none}.more summary::-webkit-details-marker{display:none}.more-menu{position:fixed;z-index:80;top:8px;left:8px;right:auto;display:flex;flex-direction:column;box-sizing:border-box;width:max-content;min-width:160px;max-width:min(240px,calc(100vw - 16px));max-height:calc(100dvh - 16px);overflow:auto;padding:5px;border:1px solid var(--line);border-radius:8px;background:var(--surface);box-shadow:0 10px 30px #0008}.more-menu button,.more-menu a{display:flex;flex:0 0 auto;align-items:center;justify-content:flex-start;box-sizing:border-box;width:100%;min-width:0;height:auto;min-height:36px;padding:8px 10px;border:0;border-radius:5px;background:transparent;color:var(--text);font:inherit;font-size:12px;line-height:1.35;text-align:left;text-decoration:none;white-space:normal;overflow-wrap:anywhere;word-break:normal}.more-menu button:hover,.more-menu a:hover{background:var(--raised)}
  @media(max-width:640px){.shared-track-list li{gap:7px}.track-position{flex-basis:18px;font-size:10px}.artwork{flex-basis:36px;width:36px;height:36px}.duration{display:none}.track-title strong{font-size:12px}.track-subtitle{font-size:10px}.row-actions{gap:2px}}
  @media(max-width:450px){.artwork{flex-basis:32px;width:32px;height:32px}.shared-track-list li{gap:5px}.icon-action,.more summary{font-size:12px}}
  @media(prefers-reduced-motion:reduce){.shared-track-list li.buffering .icon-action.play{animation:none}}
  .shared-track-list li.selected{outline:1px solid color-mix(in srgb,var(--accent) 45%,transparent);outline-offset:-1px}.select-track{flex:0 0 16px;width:16px;height:16px;margin:0;accent-color:var(--accent)}
  .icon-action{font-size:17px}.more summary{font-size:20px}
  @media(max-width:450px){.icon-action{font-size:16px}.more summary{font-size:19px}}
</style>
