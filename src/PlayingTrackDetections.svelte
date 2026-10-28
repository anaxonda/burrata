<script lang="ts">
  import { approximateDetectionTime, type DetectedTrackObservation } from "./lib/playing-track-detection";

  let {
    enabled = false,
    observations = [],
    status = "",
    disabledReason = "",
    onToggle,
    onSeek,
  }: {
    enabled?: boolean;
    observations?: DetectedTrackObservation[];
    status?: string;
    disabledReason?: string;
    onToggle: (enabled: boolean) => void;
    onSeek: (seconds: number) => void;
  } = $props();
</script>

<section class="detected-tracks" aria-label="Detected tracks in this recording">
  <div class="detection-heading">
    <div>
      <h3>Detected tracks</h3>
      <p>Identify songs inside a mix and jump back to roughly where they were heard. Saved detections are separate from supplied chapters.</p>
    </div>
  </div>
  {#if disabledReason}
    <p class="detection-status">{disabledReason}</p>
  {:else}
    <label class="detection-toggle">
      <input type="checkbox" checked={enabled} onchange={(event) => onToggle(event.currentTarget.checked)} />
      <span>Detect tracks in this recording</span>
    </label>
    <p class="detection-status">While enabled, short audio samples are sent to the recognition service.</p>
    {#if status}<p class="detection-status" aria-live="polite">{status}</p>{/if}
    {#if observations.length}
      <ol>
        {#each observations as observation (`${observation.detectedAtSeconds}:${observation.observedAt}`)}
          <li>
            <button class="detected-track" onclick={() => onSeek(observation.detectedAtSeconds)} aria-label={`Seek to approximately ${approximateDetectionTime(observation.detectedAtSeconds)}: ${observation.artist} — ${observation.title}`}>
              <span class="detection-time">Detected at {approximateDetectionTime(observation.detectedAtSeconds)}</span>
              <strong>{observation.title}</strong>
              <span>{observation.artist}</span>
            </button>
          </li>
        {/each}
      </ol>
    {:else if enabled && !status}
      <p class="detection-status">No tracks detected yet.</p>
    {/if}
  {/if}
</section>

<style>
  .detected-tracks { display:grid; gap:8px; margin:14px 0; padding:12px; border:1px solid var(--line); border-radius:12px; background:var(--surface-raised); }
  .detection-heading h3 { margin:0; font-size:13px; }
  .detection-heading p,.detection-status { margin:4px 0 0; color:var(--muted); font-size:11px; line-height:1.4; }
  .detection-toggle { display:flex; align-items:center; gap:8px; font-size:12px; }
  .detection-toggle input { accent-color:var(--accent); }
  ol { display:grid; gap:5px; margin:0; padding:0; list-style:none; }
  .detected-track { width:100%; display:grid; gap:2px; padding:8px; border:0; border-radius:8px; color:inherit; background:var(--surface); text-align:left; cursor:pointer; }
  .detected-track:hover { background:var(--surface-hover); }
  .detected-track strong { font-size:12px; }
  .detected-track > span:not(.detection-time) { color:var(--muted); font-size:11px; }
  .detection-time { color:var(--muted); font-size:10px; }
</style>
