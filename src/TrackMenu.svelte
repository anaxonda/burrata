<script lang="ts">
  import { onMount } from "svelte";
  import SourceLink from "./SourceLink.svelte";
  import type { Track } from "./lib/types";
  let { track, onClose, onNext, onQueue, onPlaylist, onFavorite, onDownload, onUpgrade = onDownload, onDetails, onSelect, onRemove, onRadio, onMoreInfo, radioBusy = false, anchor = null, playerHeight = 90 }: {
    track: Track; onClose: () => void; onNext: () => void; onQueue: () => void;
    onPlaylist: () => void; onFavorite: () => void; onDownload: () => void; onUpgrade?: () => void;
    onDetails: () => void; onSelect: () => void; onRemove?: () => void; onRadio?: () => void; onMoreInfo?: () => void; radioBusy?: boolean; anchor?: { x: number; y: number } | null; playerHeight?: number;
  } = $props();
  let dialog: HTMLDialogElement;
  let viewportWidth = $state(1024), viewportHeight = $state(768), menuHeight = $state(460);
  onMount(() => { const trigger = document.activeElement as HTMLElement | null; dialog.showModal(); return () => trigger?.focus(); });
  function action(callback: () => void) { callback(); onClose(); }
</script>
<svelte:window bind:innerWidth={viewportWidth} bind:innerHeight={viewportHeight} />
<dialog bind:this={dialog} bind:clientHeight={menuHeight} class:context-menu={Boolean(anchor)} style:left={anchor ? `${Math.max(8, Math.min(anchor.x, viewportWidth - 336))}px` : undefined} style:top={anchor ? `${Math.max(8, Math.min(anchor.y, viewportHeight - playerHeight - menuHeight - 8))}px` : undefined} class="track-menu" aria-label={`Actions for ${track.title}`} onclose={onClose} oncancel={onClose} onclick={(e) => { if (e.target === dialog) onClose(); }} onkeydown={(e) => e.stopPropagation()}>
  <header><div><strong>{track.title}</strong><small>{track.artist}</small>{#if track.localAvailability === "available" || (!track.localAvailability && track.localPath)}<span class="downloaded"><span aria-hidden="true">✓</span> Downloaded</span>{/if}</div><button aria-label="Close track actions" onclick={onClose}>×</button></header>
  <button onclick={() => action(onNext)}>Play next</button>
  <button onclick={() => action(onQueue)}>Add to queue</button>
  {#if onRadio}<button disabled={radioBusy} onclick={() => action(onRadio!)}>{radioBusy ? "Finding radio…" : "Start song radio"}</button>{/if}
  <button onclick={() => action(onPlaylist)}>Add to playlist…</button>
  <button onclick={() => action(onFavorite)}>{track.favorite ? "Remove from Liked Songs" : "Add to Liked Songs"}</button>
  <SourceLink {track} />
  <button onclick={() => action(onDownload)}>Download options…</button>
  <button class="upgrade" onclick={() => action(onUpgrade)}>Find higher-quality download</button>
  <button onclick={() => action(onDetails)}>Track information</button>
  {#if onMoreInfo}<button onclick={() => action(onMoreInfo)}>Search for more info</button>{/if}
  <button onclick={() => action(onSelect)}>Select for batch actions</button>
  {#if onRemove}<button onclick={() => action(onRemove!)}>Remove from playlist</button>{/if}
</dialog>
<style>
  .track-menu { position:fixed;inset:0;margin:auto;width:min(320px,calc(100vw - 32px));max-height:calc(100dvh - 40px);overflow:auto;padding:10px;border:1px solid var(--line);border-radius:12px;background:var(--surface);color:var(--text);box-shadow:0 20px 80px #0008; }
  .track-menu.context-menu { inset:auto;margin:0;max-height:calc(100dvh - var(--player-height,90px) - 16px); }
  .track-menu.context-menu::backdrop { background:transparent; }
  .track-menu::backdrop { background:#0006; }
  header { display:flex;align-items:center;gap:12px;padding:6px 8px 12px;border-bottom:1px solid var(--line);margin-bottom:5px; }
  header div { flex:1;min-width:0; } strong,small { display:block;overflow:hidden;text-overflow:ellipsis;white-space:nowrap; } small { color:var(--muted);margin-top:4px; } header button { background:transparent;border:0;font-size:22px; }
  .downloaded{display:flex;align-items:center;gap:5px;width:max-content;margin-top:7px;padding:3px 7px;border:1px solid color-mix(in srgb,var(--accent) 55%,var(--line));border-radius:999px;color:var(--accent);font-size:10px;font-weight:650}.downloaded span{display:grid;place-items:center;width:14px;height:14px;border-radius:50%;background:color-mix(in srgb,var(--accent) 20%,transparent)}
  .track-menu>button.upgrade{color:var(--accent);font-weight:650}
  .track-menu>button { display:block;width:100%;padding:10px 12px;text-align:left;border:0;background:transparent;border-radius:5px; }
  .track-menu>button:hover { background:var(--raised); }
</style>
