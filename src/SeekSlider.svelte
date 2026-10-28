<script lang="ts">
  import { onDestroy } from "svelte";
  import { formatSeekTime, seekPreviewTime } from "./lib/seek-preview";
  let { position, duration, disabled, onSeek }: {position:number;duration:number;disabled:boolean;onSeek:(value:number)=>void} = $props();
  let target = $state<number | null>(null);
  let hoverPosition = $state<number | null>(null);
  const previewPosition = $derived(target ?? hoverPosition);
  let dragging = false;
  let timer: ReturnType<typeof setTimeout> | undefined;
  function commit() {
    clearTimeout(timer);
    if (target !== null) onSeek(target);
    target = null;
    hoverPosition = null;
  }
  function previewSeek(event: PointerEvent) {
    if (disabled) return;
    const bounds = event.currentTarget instanceof HTMLElement ? event.currentTarget.getBoundingClientRect() : null;
    hoverPosition = bounds ? seekPreviewTime(event.clientX, bounds.left, bounds.width, duration) : null;
  }
  onDestroy(() => clearTimeout(timer));
</script>
<div class="seek-control">
<input class="seek" type="range" min="0" max={duration || 1} step="0.1"
  value={target ?? position} {disabled} aria-label="Playback position"
  aria-valuetext={`${formatSeekTime(target ?? position)} of ${formatSeekTime(duration)}`}
  title="Click or drag to seek"
  onpointermove={previewSeek}
  onpointerleave={() => { if (!dragging) hoverPosition = null; }}
  onpointerdown={(event) => { clearTimeout(timer); dragging = true; event.currentTarget.setPointerCapture(event.pointerId); }}
  oninput={(event) => { target = Number(event.currentTarget.value); clearTimeout(timer); if (!dragging) timer = setTimeout(commit, 150); }}
  onpointerup={() => { dragging = false; commit(); }}
  onlostpointercapture={() => { if (dragging) { dragging = false; commit(); } }}
  onpointercancel={() => { dragging = false; clearTimeout(timer); target = null; hoverPosition = null; }}
  onblur={() => { dragging = false; commit(); }} />
{#if previewPosition !== null && !disabled && duration > 0}
  <span class="seek-hover-guide" style={`left:${100 * previewPosition / duration}%`} aria-hidden="true"></span>
  <span class="seek-hover-time" style={`left:clamp(32px, ${100 * previewPosition / duration}%, calc(100% - 32px))`} aria-hidden="true">{formatSeekTime(previewPosition)}</span>
{/if}
</div>

<style>
  .seek-control{position:relative;display:flex;align-items:center;flex:1 1 auto;width:100%;min-width:40px;height:4px}
  .seek-control .seek{width:100%;min-width:0;margin:0;cursor:pointer}
  .seek-control .seek:disabled{cursor:default}
  .seek-hover-guide{position:absolute;top:-6px;bottom:-6px;width:1px;background:var(--text,#eef5f0);opacity:.65;pointer-events:none}
  .seek-hover-time{position:absolute;bottom:14px;transform:translateX(-50%);padding:3px 6px;border-radius:4px;background:var(--surface,#17191c);color:var(--text,#eef5f0);font-size:10px;line-height:1.2;font-variant-numeric:tabular-nums;pointer-events:none;box-shadow:0 1px 5px #0006;white-space:nowrap}
</style>
