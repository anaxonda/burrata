<script lang="ts">
  import type { Candidate, LocalAsset } from "./lib/types";
  import { candidateFormatsLabel, formatSourceSize, groupDownloadCandidates, type CandidateSort } from "./lib/download-source-chooser";
  import { parseDurationBound, type SortDirection } from "./lib/track-sort";
  import TrackDurationFilter from "./TrackDurationFilter.svelte";
  import TrackFiles from "./TrackFiles.svelte";

  let {
    candidates = [], selected = null, loading = false, onSelect,
    preferredProvider = "", providerOrder = [], existingFiles = [], preferredAssetId = null, onPreferred, onReplace,
  }: {
    candidates?: Candidate[];
    selected?: Candidate | null;
    loading?: boolean;
    onSelect: (candidate: Candidate) => void;
    preferredProvider?: string;
    providerOrder?: string[];
    existingFiles?: LocalAsset[];
    preferredAssetId?: string | null;
    onPreferred?: (assetId: string) => void;
    onReplace?: (assetId: string) => void;
  } = $props();

  let sort = $state<CandidateSort>("match");
  let direction = $state<SortDirection>("asc");
  let minimumDuration = $state("");
  let maximumDuration = $state("");
  const visibleCandidates = $derived.by(() => {
    const lower = parseDurationBound(minimumDuration), upper = parseDurationBound(maximumDuration);
    return candidates.filter(candidate => {
      if (lower === null && upper === null) return true;
      const duration = candidate.durationSeconds;
      if (typeof duration !== "number" || !Number.isFinite(duration) || duration < 0) return false;
      return (lower === null || duration >= lower) && (upper === null || duration <= upper);
    });
  });
  const groups = $derived(groupDownloadCandidates(visibleCandidates, { preferredProvider, providerOrder, sort, direction }));
  function isSelected(candidate: Candidate) {
    return selected === candidate || (selected?.provider === candidate.provider && selected.id === candidate.id);
  }
  function availabilityLabel(candidate: Candidate) {
    if (candidate.permission === "restricted") return "Restricted";
    if (candidate.source?.username) return "Peer source · unverified";
    if (candidate.url) return "Source available";
    return "Availability unknown";
  }
  function qualityDetails(candidate: Candidate) {
    const formats = candidate.formats ?? [];
    const bitrate = formats.find((format) => typeof format.bitrate === "number" && format.bitrate > 0)?.bitrate ?? candidate.source?.bitrate_kbps;
    const sampleRate = formats.find((format) => typeof format.sampleRate === "number" && format.sampleRate > 0)?.sampleRate;
    return { bitrate, sampleRate, lossless: formats.some((format) => format.lossless) };
  }
</script>

<section class="source-chooser" aria-label="Download sources">
  {#if existingFiles.length}
    <TrackFiles assets={existingFiles} {preferredAssetId} onPreferred={onPreferred} onReplace={onReplace} compact />
  {/if}

  <div class="chooser-heading">
    <div><h3>Available sources</h3><span>{visibleCandidates.length} {visibleCandidates.length === 1 ? "source" : "sources"}</span></div>
    <label>Sort <select bind:value={sort} aria-label="Sort download sources">
      <option value="quality">Quality</option><option value="match">Match</option><option value="size">Size</option><option value="availability">Availability</option><option value="duration">Duration</option>
    </select></label>
  </div>
  <div class="source-duration-tools" aria-label="Download source duration controls">
    {#if sort === "duration"}<label>Direction<select aria-label="Sort direction" bind:value={direction}><option value="asc">Ascending</option><option value="desc">Descending</option></select></label>{/if}
    <TrackDurationFilter bind:minDuration={minimumDuration} bind:maxDuration={maximumDuration} minLabel="Minimum source duration" maxLabel="Maximum source duration" />
  </div>

  {#if candidates.length === 0 && !loading}<p class="empty">No sources found yet.</p>{:else if !visibleCandidates.length && !loading}<p class="empty">No sources match these duration bounds.</p>{/if}
  {#each groups as group (group.provider)}
    <section class="provider-group" aria-label={`${group.provider} sources`}>
      <h4>{group.provider}{#if group.provider === preferredProvider}<span>Preferred</span>{/if}</h4>
      {#each group.candidates as candidate (`${candidate.provider}:${candidate.id}`)}
        {@const facts = qualityDetails(candidate)}
        <button class="candidate" class:selected={isSelected(candidate)} aria-pressed={isSelected(candidate)} onclick={() => onSelect(candidate)}>
          <span class="candidate-main">
            <strong>{candidate.title || candidate.source?.filename || group.provider}</strong>
            <span class="candidate-meta">
              {#if candidate.uploader}<span>{candidate.uploader}</span>{/if}
              {#if candidate.durationSeconds}<span>{Math.floor(candidate.durationSeconds / 60)}:{String(Math.floor(candidate.durationSeconds % 60)).padStart(2, "0")}</span>{/if}
              {#if candidateFormatsLabel(candidate)}<span>{candidateFormatsLabel(candidate)}</span>{/if}
              {#if facts.bitrate}<span>{Math.round(facts.bitrate)} kbps</span>{/if}
              {#if facts.sampleRate}<span>{(facts.sampleRate / 1000).toFixed(facts.sampleRate % 1000 ? 1 : 0)} kHz</span>{/if}
              {#if facts.lossless}<span class="lossless">Lossless listed{#if candidate.source?.kind === "soulseek"} · unverified{/if}</span>{/if}
              {#if candidate.reasons?.[0]}<span title={candidate.reasons.join(" · ")}>{candidate.reasons[0]}</span>{/if}
            </span>
          </span>
          <span class="candidate-side"><strong>{formatSourceSize(candidate.source?.size_bytes)}</strong><span>{availabilityLabel(candidate)}</span><span class="confidence">{candidate.confidence || "Match unknown"} match</span></span>
        </button>
      {/each}
    </section>
  {/each}
  {#if loading}<p class="loading" role="status"><span class="spinner" aria-hidden="true"></span>Searching for more sources…</p>{/if}
</section>

<style>
  .source-chooser{container-type:inline-size;min-width:0;display:grid;gap:10px;color:var(--text)}
  .chooser-heading{display:flex;align-items:center;justify-content:space-between;gap:10px}.chooser-heading>div{display:flex;align-items:baseline;gap:8px;min-width:0}.chooser-heading h3{margin:0;font-size:13px}.chooser-heading>div>span{color:var(--muted);font-size:11px}.chooser-heading label{display:flex;align-items:center;gap:6px;color:var(--muted);font-size:11px}.chooser-heading select{max-width:130px;min-height:34px;padding:5px 24px 5px 8px;border:1px solid var(--line);border-radius:7px;background:var(--surface);color:var(--text);font:inherit}
  .source-duration-tools{display:flex;flex-wrap:wrap;gap:7px}.source-duration-tools label{display:flex;align-items:center;gap:6px;color:var(--muted);font-size:10px}.source-duration-tools select{min-width:92px;padding:6px 8px;border:1px solid var(--line);border-radius:7px;background:var(--surface);color:var(--text);font:inherit}
  .provider-group{display:grid;gap:5px;min-width:0}.provider-group h4{display:flex;align-items:center;gap:8px;margin:4px 0 0;color:var(--muted);font-size:10px;letter-spacing:.06em;text-transform:uppercase}.provider-group h4 span{padding:2px 6px;border-radius:99px;background:color-mix(in srgb,var(--accent) 16%,transparent);color:var(--accent);font-size:9px;letter-spacing:0;text-transform:none}
  .candidate{display:flex;align-items:center;justify-content:space-between;gap:12px;width:100%;min-width:0;min-height:58px;padding:9px 10px;border:1px solid var(--line);border-radius:9px;background:var(--surface);color:var(--text);text-align:left;cursor:pointer}.candidate:hover{background:var(--raised)}.candidate.selected{border-color:var(--accent);box-shadow:inset 0 0 0 1px var(--accent);background:color-mix(in srgb,var(--accent) 7%,var(--surface))}.candidate-main,.candidate-side{display:grid;gap:4px;min-width:0}.candidate-main>strong{overflow:hidden;text-overflow:ellipsis;white-space:nowrap;font-size:12px;font-weight:600}.candidate-meta{display:flex;flex-wrap:wrap;gap:3px 8px;color:var(--muted);font-size:10px}.candidate-side{flex:0 0 auto;max-width:42%;text-align:right;color:var(--muted);font-size:10px}.candidate-side strong{color:var(--text);font-size:11px;font-weight:600}.candidate-side span{overflow:hidden;text-overflow:ellipsis;white-space:nowrap}.candidate .lossless{color:var(--accent)}.confidence{text-transform:capitalize}.empty,.loading{display:flex;align-items:center;gap:8px;margin:0;padding:11px;border:1px dashed var(--line);border-radius:8px;color:var(--muted);font-size:11px}.spinner{width:12px;height:12px;border:2px solid var(--line);border-top-color:var(--accent);border-radius:50%;animation:spin .8s linear infinite}@keyframes spin{to{transform:rotate(360deg)}}
  @container (max-width:420px){.candidate{align-items:flex-start;gap:8px;padding:9px}.candidate-side{max-width:38%;font-size:9px}.candidate-meta{font-size:9px}.chooser-heading>div{display:grid;gap:2px}.chooser-heading select{max-width:110px}}
  @media(prefers-reduced-motion:reduce){.spinner{animation:none}}
</style>
