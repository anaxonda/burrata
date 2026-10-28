<script lang="ts">
  import ActionMenu from "./ActionMenu.svelte";
  import MenuIcon from "./MenuIcon.svelte";
  let { name, anchor, playerHeight, onClose, onOpen, onDelete, onDownload, onOpenSource, onRename, onExport, onMoveToFolder, onCopy, onRefresh, onMirrorSettings, kind="playlist" }: { name: string; anchor: { x: number; y: number }; playerHeight: number; onClose: () => void; onOpen: () => void; onDelete?: () => void; onRename?:()=>void;kind?:"playlist"|"radio station"; onDownload?: () => void; onRefresh?: () => void; onExport?: () => void; onMoveToFolder?: () => void; onCopy?: () => void; onOpenSource?: () => void; onMirrorSettings?: () => void } = $props();
  function action(callback: () => void) { callback(); onClose(); }
</script>
<ActionMenu class="playlist-menu" label={`Actions for ${kind} ${name}`} title={name} closeLabel="Close playlist actions" {anchor} {playerHeight} {onClose}>
  <button class="primary-action" onclick={() => action(onOpen)}><MenuIcon name="play"/><span>Open {kind}</span></button>
  {#if onRename}<button onclick={()=>action(onRename!)}><MenuIcon name="edit"/><span>Rename</span></button>{/if}
  {#if onRefresh}<button onclick={() => action(onRefresh!)}><MenuIcon name="refresh"/><span>Refresh playlist</span></button>{/if}
  {#if onOpenSource}<button class="menu-section" title={kind === "radio station" ? "Open the station website" : onRefresh ? "Open the original playlist in your browser" : "Opens the original source. Refresh it, then use the browser extension to capture an updated import."} onclick={() => action(onOpenSource!)}><MenuIcon name="external"/><span>{kind === "radio station" ? "Open station website" : onRefresh ? "Open source" : "Open source to refresh"}</span></button>{/if}
  {#if onDownload}<button onclick={() => action(onDownload!)}><MenuIcon name="download"/><span>Download playlist</span></button>{/if}
  {#if onCopy}<button onclick={() => action(onCopy!)}><MenuIcon name="copy"/><span>Make editable copy</span></button>{/if}
  {#if onMirrorSettings}<button onclick={() => action(onMirrorSettings!)}><MenuIcon name="folder"/><span>Local M3U copy…</span></button>{/if}
  {#if onExport}<button onclick={() => action(onExport!)}><MenuIcon name="export"/><span>Export playlist</span></button>{/if}
  {#if onMoveToFolder}<button onclick={() => action(onMoveToFolder!)}><MenuIcon name="folder"/><span>Move to folder…</span></button>{/if}
  {#if onDelete}<button class="menu-section destructive" onclick={() => action(onDelete!)}><MenuIcon name="remove"/><span>{kind === "radio station" ? "Remove station" : "Delete playlist"}</span></button>{/if}
</ActionMenu>
