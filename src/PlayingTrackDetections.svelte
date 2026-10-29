<script lang="ts">
  import { approximateDetectionTime, type DetectedTrackObservation } from "./lib/playing-track-detection";

  let helpOpen = $state(false);
  let helpRoot = $state<HTMLDivElement>();

  let {
    enabled = false,
    observations = [],
    status = "",
    disabledReason = "",
    retentionLimitReached = false,
    markersEnabled = false,
    interactive = true,
    onToggle,
    onMarkersToggle,
    onSeek,
    onOpenTrack,
    onAddToPlaylist,
    onAddAllToPlaylist,
  }: {
    enabled?: boolean;
    observations?: DetectedTrackObservation[];
    status?: string;
    disabledReason?: string;
    retentionLimitReached?: boolean;
    markersEnabled?: boolean;
    interactive?: boolean;
    onToggle: (enabled: boolean) => void;
    onMarkersToggle?: (enabled: boolean) => void;
    onSeek: (seconds: number) => void;
    onOpenTrack?: (observation: DetectedTrackObservation) => void;
    onAddToPlaylist?: (observation: DetectedTrackObservation) => void;
    onAddAllToPlaylist?: (observations: DetectedTrackObservation[]) => void;
  } = $props();

  const displayStatus = $derived.by(() => {
    const value = status.trim();
    if (!value || /^Detection is off\b/i.test(value)) return "";
    return value.length > 140 ? `${value.slice(0, 137)}…` : value;
  });
</script>

<svelte:document onpointerdown={(event) => {
  if(helpOpen && event.target instanceof Node && !helpRoot?.contains(event.target))helpOpen=false;
}} onkeydown={(event) => {
  if(helpOpen && event.key==="Escape"){
    event.preventDefault();event.stopPropagation();helpOpen=false;
    helpRoot?.querySelector<HTMLButtonElement>("button")?.focus();
  }
}} />
<section class="detected-tracks" aria-label="Track detection">
  {#if interactive}<div class="detection-control-row">
    <label class="detection-toggle">
      <input type="checkbox" checked={enabled} disabled={Boolean(disabledReason)} aria-describedby={disabledReason ? "track-detection-status" : undefined} onchange={(event) => onToggle(event.currentTarget.checked)} />
      <span>Detect tracks</span>
    </label>
    <div class="detection-help" bind:this={helpRoot}>
      <button type="button" aria-label="About track detection" title="About track detection" aria-expanded={helpOpen} aria-controls="track-detection-help-copy" onclick={() => helpOpen = !helpOpen}><span aria-hidden="true">i</span></button>
      {#if helpOpen}<div id="track-detection-help-copy" class="detection-help-copy" role="note">
        <p>Detection looks for songs inside a continuous recording. Times are approximate; supplied chapters remain separate.</p>
        <p>When enabled, short audio samples are sent to the recognition service. Detection is opt-in for this recording.</p>
      </div>{/if}
    </div>
  </div>
    {#if onMarkersToggle}<label class="detection-markers-toggle" title="Show approximate song markers on the seek bar">
      <input type="checkbox" checked={markersEnabled} onchange={(event) => onMarkersToggle?.(event.currentTarget.checked)} />
      <span>Show approximate song markers</span>
    </label>{/if}
  {/if}

  {#if disabledReason}
    <p id="track-detection-status" class="detection-status" role="status">{disabledReason}</p>
  {:else if retentionLimitReached}
    <p id="track-detection-status" class="detection-status" role="status">Detection history is full; existing timestamps were kept.</p>
  {:else if displayStatus}
    <p id="track-detection-status" class="detection-status" role="status" aria-live="polite">{displayStatus}</p>
  {:else if enabled && !observations.length}
    <p id="track-detection-status" class="detection-status" role="status">No tracks detected yet.</p>
  {/if}

  {#if observations.length}
    <div class="detection-list-heading"><span>Detected tracks</span>{#if onAddAllToPlaylist}<button type="button" class="add-all" onclick={() => onAddAllToPlaylist(observations)}>Add all to playlist…</button>{/if}</div>
    <ol aria-label="Detected tracks">
      {#each observations as observation (`${observation.detectedAtSeconds}:${observation.observedAt}`)}
        <li>
          <div class="detected-track">
            <button type="button" class="detection-time" onclick={() => onSeek(observation.detectedAtSeconds)} aria-label={`Seek to approximately ${approximateDetectionTime(observation.detectedAtSeconds)}`}>{approximateDetectionTime(observation.detectedAtSeconds)}</button>
            <div class="detection-copy">
              {#if onOpenTrack}<button type="button" class="track-link" onclick={() => onOpenTrack?.(observation)}>{observation.title}</button>{:else}<strong>{observation.title}</strong>{/if}
              {#if onOpenTrack}<button type="button" class="artist-link" onclick={() => onOpenTrack?.(observation)}>{observation.artist}</button>{:else}<span class="detection-artist">{observation.artist}</span>{/if}
            </div>
            {#if onAddToPlaylist}<button type="button" class="add-one" aria-label={`Add ${observation.title} by ${observation.artist} to playlist`} title="Add to playlist…" onclick={() => onAddToPlaylist?.(observation)}>＋</button>{/if}
          </div>
        </li>
      {/each}
    </ol>
  {/if}
</section>

<style>
  .detected-tracks { display:grid; gap:5px; min-width:0; margin:8px 0; padding:8px 0; border:0; border-top:1px solid var(--line); border-radius:0; background:transparent; }
  .detection-list-heading { display:flex; align-items:center; justify-content:space-between; gap:8px; color:var(--muted); font-size:10px; font-weight:700; text-transform:uppercase; letter-spacing:.06em; }
  .add-all { padding:3px 7px; border:1px solid var(--line); border-radius:5px; background:transparent; color:var(--text); font:inherit; font-size:10px; text-transform:none; letter-spacing:0; cursor:pointer; }
  .detection-control-row { position:relative; display:flex; flex-wrap:nowrap; align-items:center; gap:8px; min-height:28px; }
  .detection-toggle { display:flex; flex-direction:row; margin:0; color:var(--text); align-items:center; gap:7px; min-height:32px; font-size:12px; font-weight:600; cursor:pointer; }
  .detection-toggle input { width:16px; height:16px; margin:0; accent-color:var(--accent); }
  .detection-toggle input:disabled { cursor:not-allowed; }
  .detection-markers-toggle { display:flex; flex-direction:row; margin:0; align-items:center; gap:6px; min-height:28px; color:var(--muted); font-size:11px; cursor:pointer; }
  .detection-markers-toggle input { flex:0 0 auto; width:14px; height:14px; margin:0; accent-color:var(--accent); }
  .detection-help { position:static; flex:0 0 auto; }
  .detection-help > button { display:grid; place-items:center; width:28px; height:28px; padding:0; border:1px solid var(--line); border-radius:50%; color:var(--muted); font-size:12px; font-weight:700; cursor:pointer; }
  .detection-help > button:focus-visible { outline:2px solid var(--accent); outline-offset:2px; }
  .detection-help-copy { position:absolute; z-index:4; top:calc(100% + 5px); right:0; width:min(260px, 100%); max-width:calc(100vw - 32px); box-sizing:border-box; padding:9px 11px; border:1px solid var(--line); border-radius:8px; background:var(--surface); box-shadow:0 8px 24px #0004; }
  .detection-help-copy p { margin:0; color:var(--muted); font-size:11px; line-height:1.45; }
  .detection-help-copy p + p { margin-top:6px; }
  .detection-status { margin:0; color:var(--muted); font-size:10px; line-height:1.35; }
  ol { display:grid; gap:2px; margin:0; padding:0; list-style:none; }
  .detected-track { width:100%; min-width:0; display:grid; grid-template-columns:38px minmax(0,1fr) 28px; align-items:center; gap:6px; padding:4px 5px; border-radius:5px; }
  .detected-track:hover { background:var(--surface-hover); }
  .detection-time { padding:0; border:0; background:transparent; color:var(--muted); font-size:10px; text-align:left; cursor:pointer; }
  .detection-time:hover { color:var(--accent); text-decoration:underline; }
  .detection-copy { display:grid; gap:2px; min-width:0; }
  .track-link,.artist-link { overflow:hidden; padding:0; border:0; background:transparent; color:inherit; font:inherit; text-align:left; text-overflow:ellipsis; white-space:nowrap; cursor:pointer; }
  .track-link { font-size:11px; font-weight:700; }
  .artist-link,.detection-artist { color:var(--muted); font-size:10px; }
  .track-link:hover,.artist-link:hover { color:var(--accent); text-decoration:underline; }
  .add-one { display:grid; place-items:center; width:26px; height:26px; padding:0; border:1px solid var(--line); border-radius:50%; background:transparent; color:var(--muted); font-size:17px; cursor:pointer; }
  .add-one:hover,.add-all:hover { border-color:var(--accent); color:var(--accent); }
  .detected-track button:focus-visible,.add-all:focus-visible { outline:2px solid var(--accent); outline-offset:1px; }
  @media (max-width:420px) { .detected-track { grid-template-columns:34px minmax(0,1fr) 26px; } }
</style>
