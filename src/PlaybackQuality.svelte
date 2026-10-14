<script lang="ts">
  import type { LocalAsset, Track } from "./lib/types";

  type StreamQuality = { codec?: string; bitrateKbps?: number; live?: boolean };
  let { track = null, asset = null, compact = false, streamQuality = null }: {
    track?: Track | null; asset?: LocalAsset | null; compact?: boolean; streamQuality?: StreamQuality | null;
  } = $props();
  const actual = $derived(asset && (!track || asset.track.id === track.id || asset.recordingId === track.id || track.assetId === asset.id || (track.localPath != null && track.localPath === asset.path)) ? asset : null);
  const liveStream = $derived(Boolean(streamQuality?.live));
  const codec = $derived(liveStream ? streamQuality?.codec?.trim() : actual?.finalCodec?.trim());
  const bitrate = $derived(liveStream ? streamQuality?.bitrateKbps : actual?.finalBitrate);
  const sourceCodec = $derived(actual?.sourceCodec?.trim() ?? "");
  const losslessCodec = $derived(codec ? /^(flac|alac|wav|aiff|ape|wv|tta|pcm)$/i.test(codec) : false);
  const lossySource = $derived(Boolean(actual && losslessCodec && /(?:mp3|mpeg|aac|opus|vorbis|wma|lossy)/i.test(sourceCodec)));
  const showLossless = $derived(losslessCodec && !lossySource);
  const hasFacts = $derived(Boolean(codec || (bitrate != null && bitrate > 0)));
  const summaryLabel = $derived(lossySource ? `${codec!.toUpperCase()} · lossy source` : showLossless ? `${codec?.toUpperCase()} · lossless` : codec?.toUpperCase() ?? "Audio");
</script>

<section class="playback-quality" class:compact style:padding={compact ? "0" : undefined} style:border={compact ? "0" : undefined} style:border-radius={compact ? "0" : undefined} style:background={compact ? "transparent" : undefined} aria-label="Playback quality">
  {#if !compact}<h3>Playback quality</h3>{/if}
  {#if liveStream}
    <p class="quality-summary" style:font-size={compact ? "10px" : undefined} style:font-weight={compact ? "400" : undefined} style:gap={compact ? "3px" : undefined}>
      <span class="quality-icon" style:font-size={compact ? "10px" : undefined} aria-hidden="true">♫</span>
      {#if hasFacts}
        <strong style:font-size={compact ? "10px" : undefined} style:font-weight={compact ? "400" : undefined}>{summaryLabel}</strong>
        {#if bitrate != null && bitrate > 0}<span>{Math.round(bitrate)} kbps</span>{/if}
        {#if streamQuality?.live}<span class="live-label">Live stream</span>{/if}
      {:else}
        <strong style:font-size={compact ? "10px" : undefined}>Quality unknown</strong>{#if streamQuality?.live}<span class="live-label">Live stream</span>{/if}
      {/if}
    </p>
  {:else if actual}
    <p class="quality-summary" style:font-size={compact ? "10px" : undefined} style:font-weight={compact ? "400" : undefined} style:gap={compact ? "3px" : undefined}>
      <span class="quality-icon" style:font-size={compact ? "10px" : undefined} aria-hidden="true">{showLossless ? "◆" : "♫"}</span>
      {#if hasFacts}
        <strong style:font-size={compact ? "10px" : undefined} style:font-weight={compact ? "400" : undefined}>{summaryLabel}</strong>
        {#if bitrate != null && bitrate > 0}<span>{Math.round(bitrate)} kbps</span>{/if}
        {#if showLossless}<span class="lossless">Lossless file</span>{/if}
      {:else}
        <strong style:font-size={compact ? "10px" : undefined}>Quality unknown</strong>
      {/if}
    </p>
    {#if sourceCodec && sourceCodec.toLowerCase() !== codec?.toLowerCase()}
      <p class="source-note">Source: {sourceCodec.toUpperCase()}{#if actual.sourceBitrate != null && actual.sourceBitrate > 0}{" · "}{Math.round(actual.sourceBitrate)} kbps{/if}</p>
    {/if}
  {:else}
    <p class="quality-summary" style:font-size={compact ? "10px" : undefined} style:font-weight={compact ? "400" : undefined} style:gap={compact ? "3px" : undefined}><span class="quality-icon" style:font-size={compact ? "10px" : undefined} aria-hidden="true">♫</span><strong style:font-size={compact ? "10px" : undefined} style:font-weight={compact ? "400" : undefined}>Quality unknown</strong></p>
    <p class="source-note">No downloaded file details are available.</p>
  {/if}
</section>

<style>
  .playback-quality{padding:11px 12px;border:1px solid var(--line);border-radius:11px;background:var(--raised);}
  h3{margin:0 0 7px;color:var(--muted);font-size:10px;font-weight:650;letter-spacing:.08em;text-transform:uppercase}
  p{margin:0}.quality-summary{display:flex;align-items:center;flex-wrap:wrap;gap:7px;font-size:12px}.quality-summary strong{font-weight:650}.quality-icon{color:var(--accent);font-size:13px}.quality-summary span:not(.quality-icon){color:var(--muted)}.quality-summary .lossless{color:var(--accent)}
  .source-note{margin-top:5px;color:var(--muted);font-size:10px}
  .live-label{color:var(--muted);font-size:10px}
  .compact{display:flex;align-items:center;min-width:0;padding:0;border:0;border-radius:0;background:transparent}
  .compact h3{display:none}
  .compact .quality-summary{gap:5px;flex-wrap:nowrap;font-size:10px}
  .compact .quality-summary strong,.compact .quality-summary span{white-space:nowrap}
  .compact .source-note{display:none}
</style>
