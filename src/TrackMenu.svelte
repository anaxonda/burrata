<script lang="ts">
  import ActionMenu from "./ActionMenu.svelte";
  import SourceLink from "./SourceLink.svelte";
  import MenuIcon from "./MenuIcon.svelte";
  import { trackSource } from "./lib/source-link";
  import type { Track } from "./lib/types";
  let { track, onClose, onNext, onQueue, onPlaylist, onFavorite, onDownload, onUpgrade: _onUpgrade, onDetails, onSelect, onRemove, onRadio, onMoreInfo, onOpenMpv, onGoToAlbum, onArtist, onOpenSource, onGoToAlbumUnavailable = false, radioBusy = false, anchor = null, playerHeight = 90 }: {
    track: Track; onClose: () => void; onNext?: () => void; onQueue?: () => void;
    onPlaylist?: () => void; onFavorite?: () => void; onDownload?: () => void; onUpgrade?: () => void;
    onDetails?: () => void; onSelect?: () => void; onArtist?: () => void; onOpenSource?: () => void; onRemove?: () => void; onRadio?: () => void; onMoreInfo?: () => void; onOpenMpv?: () => void; onGoToAlbum?: () => void; onGoToAlbumUnavailable?: boolean; radioBusy?: boolean; anchor?: { x: number; y: number } | null; playerHeight?: number;
  } = $props();
  let hasSource = $derived(Boolean(trackSource(track)) || Boolean(onOpenSource));
  function action(callback: () => void) { callback(); onClose(); }
</script>
<ActionMenu class="track-menu" label={`Actions for ${track.title}`} title={track.title} subtitle={track.artist || undefined} closeLabel="Close track actions" {anchor} {playerHeight} {onClose}>
  {#snippet badge()}{#if track.localAvailability === "available" || (!track.localAvailability && track.localPath)}<span class="downloaded"><span aria-hidden="true">✓</span> Downloaded</span>{/if}{/snippet}
  {#if onNext}<button onclick={() => action(onNext!)}><MenuIcon name="play"/><span>Play next</span></button>{/if}
  {#if onQueue}<button onclick={() => action(onQueue!)}><MenuIcon name="queue"/><span>Add to queue</span></button>{/if}
  {#if onPlaylist}<button class="primary-action" onclick={() => action(onPlaylist!)}><MenuIcon name="plus"/><span>Add to playlist…</span></button>{/if}
  {#if onFavorite}<button onclick={() => action(onFavorite!)}><MenuIcon name="heart"/><span>{track.favorite ? "Remove from Liked Songs" : "Add to Liked Songs"}</span></button>{/if}
  {#if onRadio}<button class="menu-section" disabled={radioBusy} onclick={() => action(onRadio!)}><MenuIcon name="radio"/><span>{radioBusy ? "Finding radio…" : "Start song radio"}</span></button>{/if}
  {#if onArtist}<button class="menu-section" onclick={() => action(onArtist!)}><MenuIcon name="search"/><span>Open artist</span></button>{/if}
  {#if onGoToAlbum}<button class="menu-section" onclick={() => action(onGoToAlbum!)}><MenuIcon name="album"/><span>Go to album</span></button>{/if}
  {#if !onGoToAlbum && onGoToAlbumUnavailable}<button class="menu-section" disabled aria-label="Go to album. Album link unavailable for this track." title="Album link unavailable for this track."><MenuIcon name="album"/><span>Go to album</span></button>{/if}
  {#if onOpenSource}<button class="source-action menu-section" onclick={() => action(onOpenSource!)}><MenuIcon name="external"/><span>Open source</span></button>{:else if hasSource}<div class="source-row menu-section"><SourceLink {track} trailingArrow={false} onOpened={onClose}>{#snippet leadingIcon()}<MenuIcon name="external"/>{/snippet}</SourceLink></div>{/if}
  {#if onDownload}<button onclick={() => action(onDownload!)}><MenuIcon name="download"/><span>Download options…</span></button>{/if}
  {#if onDetails}<button onclick={() => action(onDetails!)}><MenuIcon name="info"/><span>Track information</span></button>{/if}
  {#if onOpenMpv}<button onclick={()=>action(onOpenMpv!)}><MenuIcon name="window"/><span>Open in mpv</span></button>{/if}
  {#if onMoreInfo}<button onclick={() => action(onMoreInfo)}><MenuIcon name="search"/><span>Search for more info</span></button>{/if}
  {#if onSelect}<button class="menu-section" onclick={() => action(onSelect!)}><MenuIcon name="select"/><span>Select for batch actions</span></button>{/if}
  {#if onRemove}<button class="menu-section destructive" onclick={() => action(onRemove!)}><MenuIcon name="remove"/><span>Remove from playlist</span></button>{/if}
</ActionMenu>
<style>
  .downloaded{display:flex;align-items:center;gap:5px;width:max-content;margin-top:7px;padding:3px 7px;border:1px solid color-mix(in srgb,var(--accent) 55%,var(--line));border-radius:999px;color:var(--accent);font-size:10px;font-weight:650}.downloaded span{display:grid;place-items:center;width:14px;height:14px;border-radius:50%;background:color-mix(in srgb,var(--accent) 20%,transparent)}
</style>
