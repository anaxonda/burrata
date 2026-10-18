<script lang="ts">
  import { onMount } from "svelte";
  import SourceLink from "./SourceLink.svelte";
  import MenuIcon from "./MenuIcon.svelte";
  import { trackSource } from "./lib/source-link";
  import type { Track } from "./lib/types";
  let { track, onClose, onNext, onQueue, onPlaylist, onFavorite, onDownload, onUpgrade: _onUpgrade, onDetails, onSelect, onRemove, onRadio, onMoreInfo, onOpenMpv, radioBusy = false, anchor = null, playerHeight = 90 }: {
    track: Track; onClose: () => void; onNext: () => void; onQueue: () => void;
    onPlaylist: () => void; onFavorite: () => void; onDownload: () => void; onUpgrade?: () => void;
    onDetails: () => void; onSelect: () => void; onRemove?: () => void; onRadio?: () => void; onMoreInfo?: () => void; onOpenMpv?: () => void; radioBusy?: boolean; anchor?: { x: number; y: number } | null; playerHeight?: number;
  } = $props();
  let dialog: HTMLDialogElement;
  let hasSource = $derived(Boolean(trackSource(track)));
  let viewportWidth = $state(1024), viewportHeight = $state(768), menuHeight = $state(460);
  onMount(() => { const trigger = document.activeElement as HTMLElement | null; dialog.showModal(); dialog.querySelector<HTMLButtonElement>(".primary-action")?.focus(); return () => trigger?.focus(); });
  function action(callback: () => void) { callback(); onClose(); }
  function handleMenuKeydown(event: KeyboardEvent) {
    if (event.key === "Escape") { event.preventDefault(); onClose(); event.stopPropagation(); return; }
    if (event.key === "Tab") {
      const focusable = [...dialog.querySelectorAll<HTMLElement>("button:not(:disabled), a[href], input:not(:disabled), select:not(:disabled), textarea:not(:disabled), [tabindex]:not([tabindex='-1'])")]
        .filter((element) => !element.matches(":disabled") && element.tabIndex >= 0 && !element.hidden && !element.closest("[hidden], [aria-hidden='true']") && element.getClientRects().length > 0);
      const first = focusable[0], last = focusable[focusable.length - 1];
      if (event.shiftKey && document.activeElement === first && last) { event.preventDefault(); last.focus(); }
      else if (!event.shiftKey && document.activeElement === last && first) { event.preventDefault(); first.focus(); }
    }
    event.stopPropagation();
  }
</script>
<svelte:window bind:innerWidth={viewportWidth} bind:innerHeight={viewportHeight} />
<dialog bind:this={dialog} bind:clientHeight={menuHeight} class:context-menu={Boolean(anchor)} style:left={anchor ? `${Math.max(8, Math.min(anchor.x, viewportWidth - 336))}px` : undefined} style:top={anchor ? `${Math.max(8, Math.min(anchor.y, viewportHeight - playerHeight - menuHeight - 8))}px` : undefined} style:--menu-player-height={`${playerHeight}px`} class="track-menu" aria-label={`Actions for ${track.title}`} onclose={onClose} oncancel={onClose} onclick={(e) => { if (e.target === dialog) onClose(); }} onkeydown={handleMenuKeydown}>
  <header><div><strong>{track.title}</strong>{#if track.artist}<small>{track.artist}</small>{/if}{#if track.localAvailability === "available" || (!track.localAvailability && track.localPath)}<span class="downloaded"><span aria-hidden="true">✓</span> Downloaded</span>{/if}</div><button aria-label="Close track actions" onclick={onClose}>×</button></header>
  <button class="primary-action" onclick={() => action(onPlaylist)}><MenuIcon name="plus"/><span>Add to playlist…</span></button>
  <button onclick={() => action(onFavorite)}><MenuIcon name="heart"/><span>{track.favorite ? "Remove from Liked Songs" : "Add to Liked Songs"}</span></button>
  <button onclick={() => action(onQueue)}><MenuIcon name="queue"/><span>Add to queue</span></button>
  <button onclick={() => action(onNext)}><MenuIcon name="play"/><span>Play next</span></button>
  {#if onRadio}<button class="menu-section" disabled={radioBusy} onclick={() => action(onRadio!)}><MenuIcon name="radio"/><span>{radioBusy ? "Finding radio…" : "Start song radio"}</span></button>{/if}
  {#if hasSource}<div class="source-row menu-section"><SourceLink {track} trailingArrow={false} onOpened={onClose}>{#snippet leadingIcon()}<MenuIcon name="external"/>{/snippet}</SourceLink></div>{/if}
  <button onclick={() => action(onDownload)}><MenuIcon name="download"/><span>Download options…</span></button>
  <button onclick={() => action(onDetails)}><MenuIcon name="info"/><span>Track information</span></button>
  {#if onOpenMpv}<button onclick={()=>action(onOpenMpv!)}><MenuIcon name="window"/><span>Open in mpv</span></button>{/if}
  {#if onMoreInfo}<button onclick={() => action(onMoreInfo)}><MenuIcon name="search"/><span>Search for more info</span></button>{/if}
  <button class="menu-section" onclick={() => action(onSelect)}><MenuIcon name="select"/><span>Select for batch actions</span></button>
  {#if onRemove}<button class="menu-section destructive" onclick={() => action(onRemove!)}><MenuIcon name="remove"/><span>Remove from playlist</span></button>{/if}
</dialog>
<style>
  .track-menu { position:fixed;inset:8px 0 calc(var(--menu-player-height,var(--player-height,90px)) + 8px);margin:auto;width:min(320px,calc(100vw - 32px));max-height:calc(100dvh - var(--menu-player-height,var(--player-height,90px)) - 16px);overflow:auto;padding:8px;border:1px solid var(--line);border-radius:10px;background:var(--surface);color:var(--text);box-shadow:0 20px 80px #0008; }
  .track-menu.context-menu { inset:auto;margin:0;max-height:calc(100dvh - var(--menu-player-height,var(--player-height,90px)) - 16px); }
  .track-menu.context-menu::backdrop { background:transparent; }
  .track-menu::backdrop { background:#0006; }
  header { display:flex;align-items:center;gap:10px;padding:4px 8px 10px;border-bottom:1px solid var(--line);margin-bottom:4px; }
  header div { flex:1;min-width:0; } strong,small { display:block;overflow:hidden;text-overflow:ellipsis;white-space:nowrap; } small { color:var(--muted);margin-top:4px; } header button { display:grid;place-items:center;flex:0 0 34px;width:34px;height:34px;min-width:34px;padding:0;background:transparent;border:0;border-radius:8px;font-size:22px;touch-action:manipulation; }
  header strong { display:-webkit-box;white-space:normal;overflow-wrap:anywhere;-webkit-box-orient:vertical;-webkit-line-clamp:2;line-clamp:2; }
  .downloaded{display:flex;align-items:center;gap:5px;width:max-content;margin-top:7px;padding:3px 7px;border:1px solid color-mix(in srgb,var(--accent) 55%,var(--line));border-radius:999px;color:var(--accent);font-size:10px;font-weight:650}.downloaded span{display:grid;place-items:center;width:14px;height:14px;border-radius:50%;background:color-mix(in srgb,var(--accent) 20%,transparent)}
  .track-menu>button,.track-menu .source-row { display:flex;align-items:center;gap:12px;width:100%;min-height:36px;box-sizing:border-box;padding:5px 9px;text-align:left;border:0;background:transparent;border-radius:5px;touch-action:manipulation;font:inherit;font-size:13px;line-height:1.25; }
  .track-menu>button:hover,.track-menu .source-row:hover { background:var(--raised); }
  .track-menu>button.menu-section,.track-menu .source-row.menu-section { margin-top:5px;padding-top:7px;border-top:1px solid var(--line);border-top-left-radius:0;border-top-right-radius:0; }
  .track-menu>button.primary-action { margin:3px 0;background:color-mix(in srgb,var(--accent) 18%,var(--surface));color:var(--text);font-weight:650; }
  .track-menu>button.primary-action:hover { background:color-mix(in srgb,var(--accent) 28%,var(--surface)); }
  .track-menu>button.destructive { color:var(--danger,#e79a93); }
  .track-menu>button:disabled { opacity:.55; }
  .track-menu.context-menu header { gap:8px;padding:2px 4px 6px; }
  .track-menu.context-menu header>div { display:flex;align-items:center;gap:8px; }
  .track-menu.context-menu header strong { display:block;flex:1;min-width:0;overflow:hidden;text-overflow:ellipsis;white-space:nowrap;overflow-wrap:normal;-webkit-line-clamp:unset;line-clamp:unset; }
  .track-menu.context-menu header small { position:absolute;width:1px;height:1px;margin:-1px;padding:0;overflow:hidden;clip:rect(0,0,0,0);white-space:nowrap;border:0; }
  .track-menu.context-menu header button { flex-basis:28px;width:28px;height:28px;min-width:28px; }
  .track-menu.context-menu header .downloaded { flex:0 0 auto;margin-top:0; }
  .track-menu :global(svg[aria-hidden="true"]) { display:block;flex:0 0 20px;color:var(--muted); }
  .track-menu>button.primary-action :global(svg[aria-hidden="true"]) { color:var(--accent); }
  .track-menu .source-row,.track-menu .source-row.menu-section { padding:0;flex-direction:column;align-items:stretch;gap:0; }
  .track-menu .source-row :global(.source-link) { display:flex;align-items:center;gap:12px;width:100%;min-height:35px;padding:5px 9px;border:0;background:transparent;color:var(--text);font:inherit;font-size:13px;text-align:left; }
  .track-menu .source-row :global(.source-link:hover) { background:transparent; }
  .track-menu button:focus-visible,.track-menu .source-row :global(.source-link:focus-visible) { outline:2px solid var(--accent);outline-offset:1px; }
  @media (max-width:600px) { .track-menu>button,.track-menu .source-row { min-height:42px; }.track-menu .source-row :global(.source-link) { min-height:41px; } }

  @media (min-width:601px) {
    .track-menu { padding:5px;border-radius:6px;border-color:var(--context-menu-border,var(--line));background:var(--context-menu-surface,var(--surface));color:var(--context-menu-text,var(--text));box-shadow:0 8px 28px #0007; }
    .track-menu>button,.track-menu .source-row { min-height:32px;gap:10px;padding:4px 8px;border-radius:3px;color:var(--context-menu-text,var(--text));line-height:1.3; }
    .track-menu>button:hover,.track-menu .source-row:hover { background:var(--context-menu-hover,var(--raised)); }
    .track-menu>button.primary-action { margin:1px 0;background:var(--context-menu-primary,var(--raised));font-weight:600; }
    .track-menu>button.primary-action:hover { background:var(--context-menu-hover,var(--raised)); }
    .track-menu :global(svg[aria-hidden="true"]) { width:18px;height:18px;flex-basis:18px;color:var(--context-menu-muted,var(--muted)); }
    .track-menu>button.menu-section { margin-top:4px;padding-top:7px;border-top-color:var(--context-menu-border,var(--line)); }

    .track-menu .source-row.menu-section { margin-top:4px;padding:0;border-top-color:var(--context-menu-border,var(--line)); }
    .track-menu .source-row :global(.source-link) { min-height:31px;gap:10px;padding:4px 8px;color:var(--context-menu-text,var(--text)); }
    .track-menu.context-menu header { color:var(--context-menu-muted,var(--muted));font-size:11px; }
  }
</style>
