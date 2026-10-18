<script lang="ts">
  import type { LocalAsset } from "./lib/types";

  let { assets = [], onPreferred, onReplace, preferredAssetId = null, compact = false }: {
    assets?: LocalAsset[];
    onPreferred?: (assetId: string) => void;
    onReplace?: (assetId: string) => void;
    preferredAssetId?: string | null;
    compact?: boolean;
  } = $props();
  function details(asset: LocalAsset) {
    const facts = asset as LocalAsset & { sizeBytes?: number; sampleRate?: number; channels?: number; bitDepth?: number; durationSeconds?: number; bitrateOrigin?: string };
    const parts = [asset.finalCodec?.toUpperCase() || "Audio"];
    const lossless = /^(flac|alac|wav|aiff|ape|wv|tta|pcm)$/i.test(asset.finalCodec ?? "") && !/(?:mp3|mpeg|aac|opus|vorbis|wma|lossy)/i.test(asset.sourceCodec ?? "");
    if (asset.finalBitrate && asset.finalBitrate > 0) parts.push(`${/(?:average|approx|estim)/i.test(facts.bitrateOrigin ?? "") ? "~" : ""}${Math.round(asset.finalBitrate)} kbps`);
    if (facts.sampleRate && facts.sampleRate > 0) parts.push(`${(facts.sampleRate / 1000).toFixed(facts.sampleRate % 1000 ? 1 : 0)} kHz`);
    if (facts.bitDepth && facts.bitDepth > 0 && lossless) parts.push(`${facts.bitDepth}-bit`);
    if (facts.channels && facts.channels > 0) parts.push(`${facts.channels} ch`);
    if (facts.durationSeconds && facts.durationSeconds > 0) parts.push(`${Math.floor(facts.durationSeconds / 60)}:${String(Math.floor(facts.durationSeconds % 60)).padStart(2, "0")}`);
    if (facts.sizeBytes && facts.sizeBytes > 0) parts.push(facts.sizeBytes >= 1_048_576 ? `${(facts.sizeBytes / 1_048_576).toFixed(1)} MB` : `${Math.round(facts.sizeBytes / 1024)} KB`);
    return parts.join(" · ");
  }
  function fileName(path: string) { const parts=path.split(/[\\/]/).filter(Boolean); return parts[parts.length-1] || path; }
</script>

{#if assets.length}
  <section class="track-files" class:compact aria-label="Files on this device">
    <header><div><h3>On this device</h3><span>{assets.length} {assets.length === 1 ? "file" : "files"}</span></div></header>
    {#each assets as asset (asset.id)}
      <article class="file-row">
        <div class="file-copy"><strong title={fileName(asset.path)}>{fileName(asset.path)}</strong><span>{details(asset)}</span><small>{asset.processing === "complete" ? "Available" : asset.processing}</small></div>
        <div class="file-actions">
          {#if onPreferred}{#if asset.id === preferredAssetId}<button disabled aria-current="true">Current</button>{:else}<button onclick={() => onPreferred?.(asset.id)}>Use this file</button>{/if}{/if}
          {#if onReplace}<button class="replace" onclick={() => onReplace?.(asset.id)}>Replace</button>{/if}
        </div>
      </article>
    {/each}
  </section>
{/if}

<style>
  .track-files{display:grid;gap:6px;min-width:0;padding:10px;border:1px solid var(--line);border-radius:10px;background:var(--raised);color:var(--text)}header{display:flex;align-items:center;justify-content:space-between}header>div{display:flex;align-items:baseline;gap:8px}h3{margin:0;font-size:12px}header span{color:var(--muted);font-size:10px}.file-row{display:flex;align-items:center;justify-content:space-between;gap:8px;min-width:0;padding-top:6px;border-top:1px solid var(--line)}.file-copy{display:grid;gap:3px;min-width:0}.file-copy strong{overflow:hidden;text-overflow:ellipsis;white-space:nowrap;font-size:11px;font-weight:600}.file-copy span,.file-copy small{color:var(--muted);font-size:10px}.file-actions{display:flex;flex:0 0 auto;flex-wrap:wrap;justify-content:flex-end;gap:5px}.file-actions button{min-height:32px;padding:5px 8px;font-size:10px}.file-actions .replace{color:var(--accent)}.compact{padding:8px;border-radius:8px}.compact h3{font-size:11px}.compact .file-copy span{font-size:9px}
</style>
