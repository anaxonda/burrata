<script lang="ts">
  import { getContext, onMount } from "svelte";
  import { SOURCE_OPENER, type SourceOpener } from "./lib/source-opening";
  import { call } from "./lib/api";
  import type { Track } from "./lib/types";
  import MenuIcon from "./MenuIcon.svelte";
  let { stationName, stationUrl = "", streamUrl = "", compact = false, track }: {
    stationName: string;
    stationUrl?: string;
    streamUrl?: string;
    compact?: boolean;
    track?: Track;
  } = $props();
  const sourceOpener = getContext<SourceOpener | undefined>(SOURCE_OPENER);
  let popoverStyle = $state("");
  let detailsElement: HTMLDetailsElement | undefined = $state();
  function closeDisclosure(restoreFocus: boolean) {
    if (!detailsElement) return;
    detailsElement.open = false;
    if (restoreFocus) detailsElement.querySelector("summary")?.focus();
  }
  function onToggle(event: Event) {
    positionPopover(event);
  }
  function positionPopover(event: Event) {
    const details = event.currentTarget;
    if (!(details instanceof HTMLDetailsElement) || !details.open) return;
    const summary = details.querySelector("summary");
    if (!summary) return;
    const rect = summary.getBoundingClientRect();
    const width = compact ? 300 : 320;
    const left = Math.max(8, Math.min(window.innerWidth - width - 8, rect.left));
    const top = rect.top >= 150 ? rect.top - 145 : rect.bottom + 8;
    popoverStyle = `--station-info-left:${left}px;--station-info-top:${Math.max(8, Math.min(window.innerHeight - 150, top))}px`;
  }
  function openUrl(event: MouseEvent, url: string) {
    if (!sourceOpener || !track) return;
    event.preventDefault();
    void sourceOpener(track, url);
  }
  onMount(() => {
    const onPointerDown = (event: PointerEvent) => {
      if (detailsElement?.open && event.target instanceof Node && !detailsElement.contains(event.target)) closeDisclosure(false);
    };
    const onKeyDown = (event: KeyboardEvent) => {
      if (detailsElement?.open && event.key === "Escape") { event.preventDefault(); closeDisclosure(true); }
    };
    document.addEventListener("pointerdown", onPointerDown, true);
    document.addEventListener("keydown", onKeyDown);
    return () => {
      document.removeEventListener("pointerdown", onPointerDown, true);
      document.removeEventListener("keydown", onKeyDown);
    };
  });
  function safeHttpUrl(value: unknown): string {
    if (typeof value !== "string" || !value.trim()) return "";
    try {
      const url = new URL(value);
      return (url.protocol === "http:" || url.protocol === "https:") && !url.username && !url.password ? url.href : "";
    } catch { return ""; }
  }
  const safeStationUrl = $derived(safeHttpUrl(stationUrl));
  const safeStreamUrl = $derived(safeHttpUrl(streamUrl));
</script>

{#if stationName.trim()}
  <details bind:this={detailsElement} class:compact class="station-info" ontoggle={onToggle}>
    <summary aria-label={`Station information for ${stationName}`} title={`Station information for ${stationName}`}>
      <span class="station-name">{stationName}</span><span class="station-info-icon" aria-hidden="true"><MenuIcon name="info" /></span>
    </summary>
    {#if safeStationUrl || safeStreamUrl}
      <div class="station-info-popover" style={popoverStyle} role="group" aria-label={`${stationName} links`}>
        {#if safeStationUrl}<div><strong>Station website</strong><a href={safeStationUrl} target="_blank" rel="noopener noreferrer" onclick={(event)=>openUrl(event,safeStationUrl)}>{safeStationUrl}</a></div>{/if}
        {#if safeStreamUrl}<div><strong>Stream URL</strong><a href={safeStreamUrl} target="_blank" rel="noopener noreferrer" onclick={(event)=>openUrl(event,safeStreamUrl)}>{safeStreamUrl}</a></div>{/if}
      </div>
    {/if}
  </details>
{/if}

<style>
  .station-info{position:relative;display:inline-flex;max-width:100%;color:inherit;font-size:inherit;line-height:1.25}
  .station-info summary{display:inline-flex;align-items:center;gap:.35rem;max-width:100%;list-style:none;cursor:pointer}
  .station-info summary::-webkit-details-marker{display:none}
  .station-name{min-width:0;overflow:hidden;text-overflow:ellipsis;white-space:nowrap}
  .station-info-icon{display:inline-grid;place-items:center;flex:0 0 20px;width:20px;height:20px;border-radius:4px;color:var(--muted,#a3a8ae);transition:color .12s,background .12s}
  .station-info-icon :global(svg){width:14px;height:14px}
  .station-info summary:hover .station-info-icon,.station-info summary:focus-visible .station-info-icon{color:var(--text,#eee);background:var(--hover,rgba(255,255,255,.06))}
  .station-info summary:focus-visible{outline:2px solid var(--accent,#80bfff);outline-offset:2px;border-radius:3px}
  .station-info-popover{position:fixed;z-index:1000;left:var(--station-info-left,8px);top:var(--station-info-top,8px);display:grid;gap:.65rem;width:min(320px,calc(100vw - 1rem));max-width:calc(100vw - 1rem);padding:.7rem .8rem;border:1px solid var(--line,#484d54);border-radius:9px;background:var(--surface,#202328);color:var(--text,#eee);box-shadow:0 10px 32px #0009;font-size:.75rem}
  .station-info-popover>div{display:grid;gap:.25rem;min-width:0}
  .station-info-popover strong{color:var(--muted,#a3a8ae);font-size:.68rem;font-weight:650}
  .station-info-popover a{min-width:0;overflow-wrap:anywhere;color:var(--accent,#80bfff);text-decoration:underline;text-underline-offset:2px}
  .compact .station-info-popover{width:min(300px,calc(100vw - 1rem));font-size:.68rem}
</style>
