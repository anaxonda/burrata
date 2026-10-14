<script lang="ts">
  import { onMount } from "svelte";
  let { name, anchor, playerHeight, onClose, onOpen, onDelete, onDownload, onOpenSource, onRename, kind="playlist" }: { name: string; anchor: { x: number; y: number }; playerHeight: number; onClose: () => void; onOpen: () => void; onDelete?: () => void; onRename?:()=>void;kind?:"playlist"|"radio station"; onDownload?: () => void; onOpenSource?: () => void } = $props();
  let dialog: HTMLDialogElement;
  let width = $state(1024), height = $state(768), menuHeight = $state(160);
  onMount(() => { const trigger = document.activeElement as HTMLElement | null; dialog.showModal(); return () => trigger?.focus(); });
  function action(callback: () => void) { callback(); onClose(); }
</script>
<svelte:window bind:innerWidth={width} bind:innerHeight={height} />
<dialog bind:this={dialog} bind:clientHeight={menuHeight} class="playlist-menu" aria-label={`Actions for ${kind} ${name}`} style:left={`${Math.max(8,Math.min(anchor.x,width - 276))}px`} style:top={`${Math.max(8,Math.min(anchor.y,height - playerHeight - menuHeight - 8))}px`} onclose={onClose} oncancel={onClose} onclick={(e) => { if (e.target === dialog) onClose(); }} onkeydown={(e) => e.stopPropagation()}>
  <header><strong>{name}</strong><button aria-label="Close playlist actions" onclick={onClose}>×</button></header>
  <button onclick={() => action(onOpen)}>Open {kind}</button>
  {#if onRename}<button onclick={()=>action(onRename!)}>Rename</button>{/if}
  {#if onOpenSource}<button title={kind === "radio station" ? "Open the station website" : "Opens the original source. Refresh it, then use the browser extension to capture an updated import."} onclick={() => action(onOpenSource!)}>{kind === "radio station" ? "Open station website ↗" : "Open source to refresh ↗"}</button>{/if}
  {#if onDownload}<button onclick={() => action(onDownload!)}>Download playlist</button>{/if}
  {#if onDelete}<button onclick={() => action(onDelete!)}>{kind === "radio station" ? "Remove station" : "Delete playlist"}</button>{/if}
</dialog>
<style>
  .playlist-menu { position:fixed;inset:auto;margin:0;width:min(260px,calc(100vw - 32px));max-height:calc(100dvh - var(--player-height,90px) - 16px);overflow:auto;padding:10px;border:1px solid var(--line);border-radius:10px;background:var(--surface);color:var(--text);box-shadow:0 18px 50px #0008; }
  .playlist-menu::backdrop { background:transparent; } header { display:flex;align-items:center;gap:10px;padding:5px 8px 10px;border-bottom:1px solid var(--line); } strong { flex:1;min-width:0;overflow:hidden;text-overflow:ellipsis;white-space:nowrap; } header button { border:0;background:transparent;font-size:20px; } .playlist-menu>button { display:block;width:100%;text-align:left;background:transparent;border:0;padding:11px;border-radius:5px; }.playlist-menu>button:hover { background:var(--raised); }
</style>
