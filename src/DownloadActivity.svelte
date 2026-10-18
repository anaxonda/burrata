<script lang="ts">
  import { downloadStateLabel as stateLabel, downloadSourceLabel as sourceLabel, downloadPeer as peer, downloadTitle as title, downloadTransferDetail } from "./lib/download-display";
  import type { DownloadJob } from "./lib/types";

  let {
    jobs,
    onClose,
    onRefresh,
    onCancel,
    onRetry,
    onShowFile,
    tagStatuses = {},
    onReviewTags,
    onFindSource,
    onViewAll,
  }: {
    jobs: DownloadJob[];
    onClose: () => void;
    onRefresh: () => void;
    onCancel: (jobId: string) => void;
    onRetry: (jobId: string) => void;
    onShowFile: (path: string) => void;
    tagStatuses?: Record<string, { label: string; assetId: string; review: boolean }>;
    onReviewTags?: (assetId: string) => void;
    onFindSource?: (job: DownloadJob) => void;
    onViewAll?: () => void;
  } = $props();

  function fallbackFrom(job: DownloadJob): string {
    const attempts = (job.track.provenance as Record<string,unknown>)?.downloadFallbackAttempts;
    if (!Array.isArray(attempts)) return "";
    const provider = attempts[0]?.provider;
    return typeof provider === "string" && provider !== job.candidate.provider ? sourceLabel({ ...job, candidate:{...job.candidate, provider} }) : "";
  }
  const terminal = (state: string) => ["completed", "failed", "cancelled", "interrupted"].includes(state);
  const retryable = (state: string) => ["failed", "cancelled", "interrupted"].includes(state);
  const quality = (job: DownloadJob) => {
    const format = job.candidate.formats?.find((item) => item.id === job.formatId) || job.candidate.formats?.[0];
    if (!format) return job.outputProfile === "mp3" ? "MP3" : "Source quality";
    return [format.codec.toUpperCase(), format.bitrate ? `${format.bitrate} kbps` : "", format.sampleRate ? `${format.sampleRate} Hz` : "", format.lossless ? "Lossless" : ""].filter(Boolean).join(" · ");
  };
  const provenanceItems = (job: DownloadJob) => {
    const data = job.track.provenance;
    if (!data || typeof data !== "object" || Array.isArray(data)) return [];
    const record = data as Record<string, unknown>;
    return ["identity", "sourceKind", "metadataSource", "sourceUrl"].filter((key) => record[key] != null).map((key) => [key, String(record[key])]);
  };
</script>

<aside class="download-activity" aria-label="Download activity">
  <header class="activity-header">
    <div><p class="eyebrow">QUEUE</p><h2>Download activity <span>{jobs.length}</span></h2></div>
    <div class="header-controls">
      {#if onViewAll}<button class="view-all" aria-label="View all downloads" onclick={onViewAll}>View all</button>{/if}
      <button class="refresh" aria-label="Refresh downloads" title="Refresh" onclick={onRefresh}>↻</button>
      <button class="close" aria-label="Close download activity" onclick={onClose}>×</button>
    </div>
  </header>

  <div class="activity-list">
    {#each jobs as job (job.id)}
      <article class="activity-job" class:failed={job.state === "failed"}>
        <strong class="job-title">{title(job)}</strong>
        <p class="job-artist">{job.track.artist || "Artist not identified"}</p>
        {#if job.track.version}<p class="job-version">{job.track.version}</p>{/if}
        <p class="job-source">{sourceLabel(job)}{#if fallbackFrom(job)} <span>·</span> Fallback from {fallbackFrom(job)}{/if}{#if peer(job)} <span>·</span> {peer(job)}{/if}</p>
        <div class="job-topline">
          <span class="state-dot" class:done={job.state === "completed"} class:error={job.state === "failed"}></span>
          <span class="state-label">{stateLabel(job.state)}</span>
          {#if job.state === "downloading"}<span class="progress-label">{Math.round(job.progress)}%</span>{/if}
        </div>
        {#if downloadTransferDetail(job)}<p class="job-source">{downloadTransferDetail(job)}</p>{/if}
        {#if tagStatuses[job.id]}<p class="job-subtitle">{tagStatuses[job.id].label}{#if tagStatuses[job.id].review}<button onclick={()=>onReviewTags?.(tagStatuses[job.id].assetId)}>Review tags</button>{/if}</p>{/if}
        {#if job.state === "downloading"}
          <progress max="100" value={Math.max(0, Math.min(100, job.progress))} aria-label={`${job.track.title} download progress`}></progress>
        {/if}
        {#if job.error}<p class="job-error">{job.error}</p>{/if}

        <details class="job-details">
          <summary>More details</summary>
          <div class="detail-grid">
            <span>Source</span><strong>{job.candidate.provider}{job.candidate.uploader ? ` · ${job.candidate.uploader}` : ""}</strong>
            <span>Quality</span><strong>{quality(job)}</strong>
            <span>Match</span><strong>{job.candidate.confidence}{job.candidate.score ? ` · score ${Math.round(job.candidate.score)}` : ""}</strong>
            {#each provenanceItems(job) as [key, value]}
              <span>{key}</span><strong title={value}>{value.length > 100 ? `${value.slice(0, 97)}…` : value}</strong>
            {/each}
          </div>
          {#if job.candidate.reasons?.length}<p class="match-reasons">{job.candidate.reasons.slice(0, 2).join(" · ")}</p>{/if}
        </details>

        <div class="job-actions">
          {#if onFindSource && job.candidate.provider === "slskd" && ["waiting_for_peer", "stalled", "failed", "interrupted"].includes(job.state)}<button onclick={() => onFindSource?.(job)}>Find another source</button>{/if}
          {#if retryable(job.state)}<button class="retry" onclick={() => onRetry(job.id)}>Retry</button>{:else if job.state !== "completed"}<button onclick={() => onCancel(job.id)}>Cancel</button>{/if}
          {#if job.outputPath}<button class="show-file" onclick={() => onShowFile(job.outputPath!)}>Show file</button>{/if}
        </div>
      </article>
    {:else}
      <div class="activity-empty"><span aria-hidden="true">↓</span><strong>No downloads yet</strong><p>Downloads you start will appear here.</p></div>
    {/each}
  </div>
</aside>

<style>
  .download-activity {
    position: fixed;
    z-index: 40;
    top: 12px;
    right: 12px;
    bottom: calc(var(--player-height, 92px) + 12px + env(safe-area-inset-bottom));
    display: flex;
    flex-direction: column;
    width: min(390px, calc(100vw - 24px));
    min-height: 0;
    overflow: hidden;
    border: 1px solid var(--line);
    border-radius: 13px;
    background: var(--surface);
    color: var(--text);
    box-shadow: 0 18px 60px #0009;
  }
  .activity-header { display: flex; flex-wrap: wrap; flex: 0 0 auto; align-items: center; justify-content: space-between; gap: 12px; padding: 16px 17px 13px; border-bottom: 1px solid var(--line); }
  .activity-header .eyebrow { margin: 0 0 5px; font-size: 9px; }
  .activity-header h2 { display: flex; align-items: center; gap: 8px; margin: 0; font-size: 16px; letter-spacing: -.35px; }
  .activity-header h2 span { display: grid; place-items: center; min-width: 20px; height: 20px; padding: 0 5px; border-radius: 10px; background: var(--raised); color: var(--muted); font-size: 10px; font-weight: 550; }
  .header-controls { display: flex; gap: 5px; margin-left: auto; }
  .header-controls button { display: grid; place-items: center; width: 32px; height: 32px; padding: 0; border-radius: 8px; background: transparent; color: var(--muted); font-size: 17px; }
  .header-controls .refresh { font-size: 20px; }
  .header-controls .view-all { width: auto; min-width: 72px; padding: 0 9px; font-size: 10px; }
  .activity-list { min-height: 0; flex: 1; overflow-y: auto; padding: 8px 11px 14px; overscroll-behavior: contain; }
  .activity-job { margin: 0; padding: 16px 6px; border-bottom: 1px solid var(--line); }
  .job-topline { display: flex; align-items: center; gap: 7px; margin: 8px 0; }
  .state-dot { flex: 0 0 7px; width: 7px; height: 7px; border-radius: 50%; background: var(--muted); }
  .state-dot.done { background: #76cf8b; }
  .state-dot.error { background: #ef927d; }
  .state-label,.progress-label { color: var(--muted); font-size: 10px; }
  .progress-label { margin-left: auto; color: var(--muted); font-variant-numeric: tabular-nums; }
  .job-title { display: block; color: var(--text); font-size: 13px; font-weight: 600; line-height: 1.45; overflow-wrap: anywhere; }
  .job-artist { margin: 4px 0; color: var(--text); font-size: 12px; line-height: 1.4; }
  .job-version,.job-source { margin: 3px 0; color: var(--muted); font-size: 11px; line-height: 1.4; overflow-wrap: anywhere; }
  .job-subtitle { overflow: hidden; margin: 3px 0 9px; color: var(--muted); font-size: 11px; text-overflow: ellipsis; white-space: nowrap; }
  progress { display: block; width: 100%; height: 4px; overflow: hidden; border: 0; border-radius: 4px; appearance: none; background: var(--line); }
  progress::-webkit-progress-bar { border-radius: 4px; background: var(--line); }
  progress::-webkit-progress-value { border-radius: 4px; background: var(--accent); }
  progress::-moz-progress-bar { border-radius: 4px; background: var(--accent); }
  .job-error { margin: 8px 0 0; color: #efa492; font-size: 10px; line-height: 1.45; overflow-wrap: anywhere; }
  .job-details { margin-top: 9px; color: var(--muted); }
  .job-details summary { width: fit-content; font-size: 10px; cursor: pointer; }
  .detail-grid { display: grid; grid-template-columns: 54px minmax(0,1fr); gap: 6px 8px; margin-top: 9px; font-size: 10px; }
  .detail-grid > span { color: var(--muted); }
  .detail-grid strong { overflow: hidden; color: var(--text); font-weight: 500; text-overflow: ellipsis; }
  .match-reasons { margin: 8px 0 0; color: var(--muted); font-size: 10px; line-height: 1.4; }
  .job-actions { display: flex; justify-content: flex-end; gap: 7px; margin-top: 9px; }
  .job-actions button { min-height: 29px; padding: 5px 10px; font-size: 10px; }
  .job-actions .retry { border-color: color-mix(in srgb, var(--accent) 50%, var(--line)); color: var(--accent); }
  .job-actions .show-file { border-color: transparent; background: transparent; color: var(--muted); }
  .activity-empty { display: grid; justify-items: center; align-content: center; min-height: 220px; height: 100%; color: var(--muted); text-align: center; }
  .activity-empty > span { display: grid; place-items: center; width: 44px; height: 44px; margin-bottom: 13px; border-radius: 14px; background: var(--raised); color: var(--accent); font-size: 24px; }
  .activity-empty strong { color: var(--text); font-size: 13px; font-weight: 570; }
  .activity-empty p { margin: 6px 0 0; font-size: 11px; }
  @media (max-width: 767px) {
    .download-activity { top: 8px; right: 8px; bottom: calc(var(--player-height, 100px) + 8px + env(safe-area-inset-bottom)); left: 8px; width: auto; max-height: calc(100dvh - var(--player-height, 100px) - 16px - env(safe-area-inset-bottom)); border-radius: 14px; }
    .activity-header { padding: 13px 15px 11px; }
    .activity-list { padding: 7px 10px 12px; }
  }
</style>
