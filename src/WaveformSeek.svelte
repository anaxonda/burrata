<script lang="ts">
  import { onDestroy } from "svelte";
  import SeekSlider from "./SeekSlider.svelte";
  import { formatSeekTime as timeLabel, seekPreviewTime } from "./lib/seek-preview";
  import { waveformStatus, type WaveformSnapshot } from "./lib/waveform-data";

  let { position, duration, disabled, onSeek, waveform = null, generation, trackId }: {
    position: number;
    duration: number;
    disabled: boolean;
    onSeek: (value: number) => void;
    waveform?: WaveformSnapshot | null;
    generation: number | null;
    trackId: string | null;
  } = $props();

  let target = $state<number | null>(null);
  let hoverPosition = $state<number | null>(null);
  let dragging = false;
  let timer: ReturnType<typeof setTimeout> | undefined;
  const waveformId = `waveform-status-${Math.random().toString(36).slice(2)}`;

  const hasTimeline = $derived(Number.isFinite(duration) && duration > 0);
  const safePosition = $derived(hasTimeline && Number.isFinite(position)
    ? Math.max(0, Math.min(duration, position))
    : 0);
  function isUsableBin(bin: WaveformSnapshot["bins"][number]): bin is NonNullable<WaveformSnapshot["bins"][number]> {
    return Boolean(bin && Number.isFinite(bin.min) && Number.isFinite(bin.max) &&
      bin.min >= -1 && bin.max <= 1 && bin.min <= bin.max);
  }
  const current = $derived.by(() => {
    if (!waveform || generation === null || trackId === null || waveform.generation !== generation || waveform.trackId !== trackId) return null;
    if (!hasTimeline || Math.abs(waveform.durationSeconds - duration) > 0.25 || !waveform.bins.some(isUsableBin)) return null;
    return waveform;
  });
  // Scale quiet files for display only. Missing bins stay missing, and this
  // bounded multiplier never changes the audio or the stored waveform data.
  const displayScale = $derived.by(() => {
    if (!current) return 1;
    let peak = 0;
    for (const bin of current.bins) {
      if (isUsableBin(bin)) peak = Math.max(peak, Math.abs(bin.min), Math.abs(bin.max));
    }
    return peak > 0 ? Math.min(14, 0.88 / peak) : 1;
  });
  const displayAmplitude = (value: number) => Math.max(-1, Math.min(1, value * displayScale));
  const unknownRailPath = $derived.by(() => {
    if (!current) return "";
    const segments: string[] = [];
    let runStart: number | null = null;
    for (let index = 0; index <= current.bins.length; index++) {
      const observed = index < current.bins.length && isUsableBin(current.bins[index]!);
      if (!observed && runStart === null) runStart = index;
      if ((observed || index === current.bins.length) && runStart !== null) {
        segments.push(`M${runStart} 91H${index}`);
        runStart = null;
      }
    }
    return segments.join("");
  });
  const elapsedEnd = $derived(current ? current.bins.length * safePosition / duration : 0);
  const previewPosition = $derived(target ?? hoverPosition);
  function previewSeek(event: PointerEvent) {
    if (disabled) return;
    const bounds = event.currentTarget instanceof HTMLElement ? event.currentTarget.getBoundingClientRect() : null;
    if (!bounds || bounds.width <= 0) return;
    hoverPosition = seekPreviewTime(event.clientX, bounds.left, bounds.width, duration);
  }
  const status = $derived.by(() => {
    if (!hasTimeline) return "Playback timeline unavailable because the recording duration is unknown.";
    if (current) return waveformStatus(current);
    return "Waveform samples are unavailable for this playback position. Standard seek bar shown.";
  });

  function commit() {
    clearTimeout(timer);
    if (target !== null) onSeek(Math.max(0, Math.min(duration || 1, target)));
    target = null;
    hoverPosition = null;
  }
  function scheduleCommit() {
    clearTimeout(timer);
    if (!dragging) timer = setTimeout(commit, 150);
  }
  function cancel() { clearTimeout(timer); target = null; hoverPosition = null; dragging = false; }
  onDestroy(() => clearTimeout(timer));
</script>

{#if !hasTimeline}
  <div class="waveform-seek-frame waveform-unavailable" title={status}>
    <input class="waveform-disabled-range" type="range" min="0" max="1" value="0" disabled
      aria-label="Playback position unavailable" aria-valuetext="Duration unavailable" aria-describedby={waveformId} />
    <span class="waveform-unavailable-label" aria-hidden="true">Timeline unavailable</span>
    <span class="waveform-status" id={waveformId} role="status">{status}</span>
  </div>
{:else if !current}
  <div class="waveform-seek-frame waveform-standard">
    <SeekSlider position={safePosition} duration={duration} {disabled} {onSeek} />
    <span class="waveform-status" id={waveformId} role="status">{status}</span>
  </div>
{:else}
<div class="waveform-seek-frame">
  <div class="waveform-seek" class:empty={!current} class:disabled>
    <div class="waveform-played-region" style={`width:${100 * safePosition / duration}%`} aria-hidden="true"></div>
    <svg class="waveform" viewBox={`0 0 ${current?.bins.length ?? 1} 100`} preserveAspectRatio="none" aria-hidden="true">
      {#if unknownRailPath}<path class="waveform-unknown-rail" d={unknownRailPath} />{/if}
      <line x1="0" y1="50" x2={current?.bins.length ?? 1} y2="50" />
      {#if current}
        {#each current.bins as bin, index (index)}
          {#if isUsableBin(bin)}
            {@const top = Math.max(0, Math.min(100, 50 - displayAmplitude(bin.max) * 46))}
            {@const bottom = Math.max(0, Math.min(100, 50 - displayAmplitude(bin.min) * 46))}
            {@const upper = Math.min(top, bottom)}
            {@const lower = Math.max(top, bottom)}
            <rect x={index + 0.15} y={upper} width="0.7" height={Math.max(1, lower - upper)} />
          {/if}
      {/each}
      {/if}
      <line class="waveform-elapsed-rail" x1="0" y1="97" x2={elapsedEnd} y2="97" />
    </svg>
    <input class="waveform-range" type="range" min="0" max={duration} step="0.1"
      value={target ?? safePosition} {disabled} aria-label="Playback position, waveform" aria-describedby={waveformId}
      aria-valuetext={`${timeLabel(target ?? safePosition)} of ${timeLabel(duration)}`}
      title={`Click or drag anywhere in this lane to seek. ${status}`}
      onpointermove={previewSeek}
      onpointerleave={() => { if (!dragging) hoverPosition = null; }}
      onpointerdown={(event) => { clearTimeout(timer); dragging = true; event.currentTarget.setPointerCapture(event.pointerId); }}
      oninput={(event) => { target = Number(event.currentTarget.value); scheduleCommit(); }}
      onpointerup={() => { dragging = false; commit(); }}
      onlostpointercapture={() => { if (dragging) { dragging = false; commit(); } }}
      onpointercancel={cancel}
      onblur={commit}
    />
    {#if previewPosition !== null && !disabled}
      <span class="waveform-hover-guide" style={`left:${100 * previewPosition / duration}%`} aria-hidden="true"></span>
      <span class="waveform-hover-time" style={`left:clamp(32px, ${100 * previewPosition / duration}%, calc(100% - 32px))`} aria-hidden="true">{timeLabel(previewPosition)}</span>
    {/if}
  </div>
  <span class="waveform-status sr-only" id={waveformId}>{status}</span>
</div>
{/if}

<style>
  .waveform-seek-frame{position:relative;isolation:isolate;flex:1 1 auto;width:100%;height:2rem;min-width:40px;max-width:none}
  .waveform-seek{position:relative;isolation:isolate;contain:layout paint;overflow:hidden;width:100%;height:2rem;display:grid;grid-template:2rem / minmax(0,1fr);align-items:center;touch-action:pan-y;border-radius:5px;background:color-mix(in srgb,var(--text,#eef5f0) 7%,transparent);box-shadow:inset 0 0 0 1px color-mix(in srgb,var(--muted,#9ca6a0) 35%,transparent);transition:background 120ms,box-shadow 120ms}
  .waveform-seek:not(.disabled):hover{background:color-mix(in srgb,var(--text,#eef5f0) 11%,transparent);box-shadow:inset 0 0 0 1px color-mix(in srgb,var(--accent,#71bda2) 65%,transparent)}
  .waveform-played-region{position:absolute;inset:0 auto 0 0;background:color-mix(in srgb,var(--accent,#71bda2) 12%,transparent);pointer-events:none}
  .waveform-hover-guide{position:absolute;inset-block:0;width:1px;background:var(--text,#eef5f0);opacity:.65;pointer-events:none}
  .waveform-hover-time{position:absolute;top:2px;transform:translateX(-50%);padding:2px 5px;border-radius:3px;background:var(--surface,#17191c);color:var(--text,#eef5f0);font-size:10px;line-height:1.2;font-variant-numeric:tabular-nums;pointer-events:none;box-shadow:0 1px 4px #0006}
  .waveform{position:relative;grid-area:1 / 1;width:100%;height:2rem;display:block;overflow:hidden;pointer-events:none}
  .waveform line{stroke:var(--line,#69726f);stroke-width:1;opacity:.9}
  .waveform .waveform-unknown-rail{fill:none;stroke:var(--muted,#9ca6a0);stroke-width:2;stroke-dasharray:2 2.8;stroke-linecap:round;opacity:.58;pointer-events:none}
  .waveform .waveform-elapsed-rail{stroke:var(--accent,#71bda2);stroke-width:2.5;stroke-linecap:round;opacity:.95}
  .waveform rect{fill:var(--accent,#71bda2);stroke:var(--text,#eef5f0);stroke-opacity:.2;stroke-width:.28;opacity:1}
  .waveform-seek.empty .waveform rect{display:none}
  .waveform-range{position:relative;grid-area:1 / 1;box-sizing:border-box;min-width:0;min-height:0;width:100%;height:2rem;margin:0;padding:0;border:0;appearance:none;-webkit-appearance:none;background:transparent;cursor:pointer}
  .waveform-range:disabled{cursor:default;opacity:.55}
  .waveform-range:focus-visible{outline:2px solid var(--accent,#71bda2);outline-offset:-2px;border-radius:5px}
  .waveform-range::-webkit-slider-runnable-track{height:2rem;background:transparent}
  .waveform-range::-moz-range-track{height:2rem;background:transparent}
  .waveform-range::-webkit-slider-thumb{appearance:none;-webkit-appearance:none;box-sizing:border-box;width:3px;height:2rem;border:0;border-radius:2px;background:var(--text,#eef5f0);box-shadow:0 0 0 1px #1118}
  .waveform-range::-moz-range-thumb{box-sizing:border-box;width:3px;height:2rem;border:0;border-radius:2px;background:var(--text,#eef5f0);box-shadow:0 0 0 1px #1118}
  .waveform-standard :global(input.seek){box-sizing:border-box;width:100%;min-width:0;max-width:none;margin:0}
  .waveform-standard{display:flex;align-items:center}
  .waveform-unavailable{display:grid;grid-template:2rem / minmax(0,1fr);align-items:center}
  .waveform-disabled-range{grid-area:1 / 1;box-sizing:border-box;width:100%;height:2rem;margin:0;appearance:none;-webkit-appearance:none;background:transparent;opacity:.48;cursor:default}
  .waveform-disabled-range::-webkit-slider-runnable-track{height:4px;border-radius:3px;background:var(--line,#69726f)}
  .waveform-disabled-range::-moz-range-track{height:4px;border-radius:3px;background:var(--line,#69726f)}
  .waveform-disabled-range::-webkit-slider-thumb{appearance:none;-webkit-appearance:none;width:3px;height:2rem;border:0;border-radius:2px;background:var(--muted,#9ca6a0)}
  .waveform-disabled-range::-moz-range-thumb{width:3px;height:2rem;border:0;border-radius:2px;background:var(--muted,#9ca6a0)}
  .waveform-unavailable-label{grid-area:1 / 1;justify-self:start;margin-left:8px;padding:0 5px;color:var(--muted,#a3a8ae);background:var(--panel,#17191c);font-size:10px;line-height:1.2;pointer-events:none}
  .waveform-status{position:absolute;width:1px;height:1px;padding:0;margin:-1px;overflow:hidden;clip:rect(0,0,0,0);white-space:nowrap;border:0}
</style>
