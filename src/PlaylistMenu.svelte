<script lang="ts">
  import { onMount } from "svelte";
  import MenuIcon from "./MenuIcon.svelte";
  let { name, anchor, playerHeight, onClose, onOpen, onDelete, onDownload, onOpenSource, onRename, onExport, onCopy, onRefresh, kind="playlist" }: { name: string; anchor: { x: number; y: number }; playerHeight: number; onClose: () => void; onOpen: () => void; onDelete?: () => void; onRename?:()=>void;kind?:"playlist"|"radio station"; onDownload?: () => void; onRefresh?: () => void; onExport?: () => void; onCopy?: () => void; onOpenSource?: () => void } = $props();
  let dialog: HTMLDialogElement;
  let width = $state(1024), height = $state(768), menuHeight = $state(160);
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
<svelte:window bind:innerWidth={width} bind:innerHeight={height} onpointerdown={(event) => { if (dialog?.open && !dialog.contains(event.target as Node)) onClose(); }} />
<dialog bind:this={dialog} bind:clientHeight={menuHeight} class="playlist-menu" aria-label={`Actions for ${kind} ${name}`} style:left={`${Math.max(8,Math.min(anchor.x,width - 336))}px`} style:top={`${Math.max(8,Math.min(anchor.y,height - playerHeight - menuHeight - 8))}px`} onclose={onClose} oncancel={onClose} onclick={(e) => { if (e.target === dialog) onClose(); }} onkeydown={handleMenuKeydown}>
  <header><strong>{name}</strong><button aria-label="Close playlist actions" onclick={onClose}>×</button></header>
  <button class="primary-action" onclick={() => action(onOpen)}><MenuIcon name="play"/><span>Open {kind}</span></button>
  {#if onRename}<button onclick={()=>action(onRename!)}><MenuIcon name="edit"/><span>Rename</span></button>{/if}
  {#if onRefresh}<button onclick={() => action(onRefresh!)}><MenuIcon name="refresh"/><span>Refresh playlist</span></button>{/if}
  {#if onOpenSource}<button class="menu-section" title={kind === "radio station" ? "Open the station website" : onRefresh ? "Open the original playlist in your browser" : "Opens the original source. Refresh it, then use the browser extension to capture an updated import."} onclick={() => action(onOpenSource!)}><MenuIcon name="external"/><span>{kind === "radio station" ? "Open station website" : onRefresh ? "Open source" : "Open source to refresh"}</span></button>{/if}
  {#if onDownload}<button onclick={() => action(onDownload!)}><MenuIcon name="download"/><span>Download playlist</span></button>{/if}
  {#if onCopy}<button onclick={() => action(onCopy!)}><MenuIcon name="copy"/><span>Make editable copy</span></button>{/if}
  {#if onExport}<button onclick={() => action(onExport!)}><MenuIcon name="export"/><span>Export playlist</span></button>{/if}
  {#if onDelete}<button class="menu-section destructive" onclick={() => action(onDelete!)}><MenuIcon name="remove"/><span>{kind === "radio station" ? "Remove station" : "Delete playlist"}</span></button>{/if}
</dialog>
<style>
  .playlist-menu { position:fixed;z-index:100;inset:auto;margin:0;width:min(320px,calc(100vw - 32px));max-height:calc(100dvh - var(--player-height,90px) - 16px);overflow:auto;padding:8px;border:1px solid var(--line);border-radius:10px;background:var(--surface);color:var(--text);box-shadow:0 20px 80px #0008; }
  .playlist-menu::backdrop { background:transparent; }
  header { display:flex;align-items:center;gap:10px;padding:4px 8px 10px;border-bottom:1px solid var(--line);margin-bottom:4px; }
  strong { flex:1;min-width:0;white-space:normal;overflow-wrap:anywhere; }
  header button { display:grid;place-items:center;flex:0 0 34px;width:34px;height:34px;min-width:34px;padding:0;border:0;border-radius:8px;background:transparent;font-size:22px; }
  .playlist-menu>button { display:flex;align-items:center;gap:12px;box-sizing:border-box;width:100%;min-height:36px;padding:5px 9px;border:0;border-radius:5px;background:transparent;color:var(--text);font:inherit;font-size:13px;line-height:1.25;text-align:left;white-space:normal;overflow-wrap:anywhere;touch-action:manipulation; }
  .playlist-menu header { gap:8px;padding:2px 4px 6px;margin-bottom:4px; }
  .playlist-menu header strong { overflow:hidden;text-overflow:ellipsis;white-space:nowrap;overflow-wrap:normal; }
  .playlist-menu header button { flex-basis:28px;width:28px;height:28px;min-width:28px; }
  .playlist-menu>button:hover { background:var(--raised); }
  .playlist-menu>button.menu-section { margin-top:5px;padding-top:7px;border-top:1px solid var(--line);border-top-left-radius:0;border-top-right-radius:0; }
  .playlist-menu>button.primary-action { margin:3px 0;background:color-mix(in srgb,var(--accent) 18%,var(--surface));font-weight:650; }
  .playlist-menu>button.primary-action:hover { background:color-mix(in srgb,var(--accent) 28%,var(--surface)); }
  .playlist-menu>button.destructive { color:var(--danger,#e79a93); }
  .playlist-menu :global(svg[aria-hidden="true"]) { display:block;flex:0 0 20px;color:var(--muted); }
  .playlist-menu>button.primary-action :global(svg[aria-hidden="true"]) { color:var(--accent); }
  .playlist-menu button:focus-visible { outline:2px solid var(--accent);outline-offset:1px; }
  @media (max-width:600px) { .playlist-menu>button { min-height:42px; } }

  @media (min-width:601px) {
    .playlist-menu { padding:5px;border-radius:6px;border-color:var(--context-menu-border,var(--line));background:var(--context-menu-surface,var(--surface));color:var(--context-menu-text,var(--text));box-shadow:0 8px 28px #0007; }
    .playlist-menu>button { min-height:32px;gap:10px;padding:4px 8px;border-radius:3px;color:var(--context-menu-text,var(--text));line-height:1.3; }
    .playlist-menu>button:hover { background:var(--context-menu-hover,var(--raised)); }
    .playlist-menu>button.primary-action { margin:1px 0;background:var(--context-menu-primary,var(--raised));font-weight:600; }
    .playlist-menu>button.primary-action:hover { background:var(--context-menu-hover,var(--raised)); }
    .playlist-menu :global(svg[aria-hidden="true"]) { width:18px;height:18px;flex-basis:18px;color:var(--context-menu-muted,var(--muted)); }
    .playlist-menu>button.menu-section { margin-top:4px;padding-top:7px;border-top-color:var(--context-menu-border,var(--line)); }
    .playlist-menu header { color:var(--context-menu-muted,var(--muted));font-size:11px; }
  }
</style>
