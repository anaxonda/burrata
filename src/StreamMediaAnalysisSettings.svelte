<script lang="ts">
  import StreamAnalysisAction from "./StreamAnalysisAction.svelte";
  import StreamMediaAnalysisAction, { type StreamMediaAnalysisActionState } from "./StreamMediaAnalysisAction.svelte";
  import type { StreamAnalysisActionState } from "./StreamAnalysisAction.svelte";
  let {
    enabled, status = "", onEnabledChange,
    mediaEligible = false, mediaState = "idle", mediaStatus = "", onMediaStart = () => {}, onMediaCancel = () => {},
    observedEligible = false, observedState = "idle", observedStatus = "", observedResult = "", cachedResult = "", onObservedStart = () => {}, onObservedCancel = () => {},
    localEligible = false, localBusy = false, localStatus = "", onLocalStart = () => {}, onLocalCancel = () => {},
  }: {
    enabled: boolean; status?: string; onEnabledChange: (enabled: boolean) => void;
    mediaEligible?: boolean; mediaState?: StreamMediaAnalysisActionState; mediaStatus?: string;
    onMediaStart?: () => void; onMediaCancel?: () => void;
    observedEligible?: boolean; observedState?: StreamAnalysisActionState; observedStatus?: string; observedResult?: string; cachedResult?: string;
    onObservedStart?: () => void; onObservedCancel?: () => void;
    localEligible?: boolean; localBusy?: boolean; localStatus?: string;
    onLocalStart?: () => void; onLocalCancel?: () => void;
  } = $props();
</script>

<label class="settings-option stream-media-analysis-setting">
  <input
    type="checkbox"
    aria-label="Analyze full streamed tracks in background"
    checked={enabled}
    onchange={(event) => onEnabledChange(event.currentTarget.checked)}
  />
  <span>
    <strong>Analyze full streamed tracks in background</strong>
    <small>When enabled, one eligible playing stream at a time is decoded locally in a separate pass (up to 15 minutes) for BPM, key, and a complete waveform. Playback continues; this uses network and CPU. No audio is saved and track metadata is unchanged.</small>
  </span>
</label>

{#if status && !(mediaEligible || mediaState !== "idle")}<p class="analysis-status" role="status">{status}</p>{/if}
{#if mediaEligible || mediaState !== "idle"}
  <div class="stream-media-analysis-control">
    <StreamMediaAnalysisAction eligible={mediaEligible} state={mediaState} status={mediaStatus || status} onStart={onMediaStart} onCancel={onMediaCancel} />
    {#if mediaStatus || status}<small role="status">{mediaStatus || status}</small>{/if}
  </div>
{/if}
{#if observedEligible || observedState !== "idle"}
  <div class="stream-media-analysis-control">
    <StreamAnalysisAction eligible={observedEligible} state={observedState} onStart={onObservedStart} onCancel={onObservedCancel} />
    {#if observedStatus}<small role="status">{observedStatus}</small>{/if}
    {#if observedResult}<small>{observedResult}</small>{/if}
  </div>
{/if}
{#if cachedResult}<div class="stream-media-analysis-control"><small role="status">{cachedResult}</small></div>{/if}
{#if localEligible}
  <div class="local-waveform-analysis-control">
    <div class="local-waveform-analysis-copy">
      <strong>Local file waveform</strong>
      <small>Visible local files are analyzed automatically when a full waveform is needed. Completed results are cached and reused; this control starts analysis now when no result is available.</small>
    </div>
    <div class="local-waveform-analysis-action">
      {#if localBusy}<button type="button" aria-label="Cancel waveform analysis" onclick={onLocalCancel}>Cancel waveform analysis</button>
      {:else}<button type="button" aria-label="Analyze full local waveform" onclick={onLocalStart}>{localStatus === "Full local waveform ready." ? "Full waveform ready" : "Analyze full file waveform"}</button>{/if}
      {#if localStatus}<small role="status">{localStatus}</small>{/if}
    </div>
  </div>
{/if}

<style>
  .stream-media-analysis-control{display:flex;align-items:center;gap:.65rem;flex-wrap:wrap;margin:.35rem 0 .65rem 2.15rem}
  .stream-media-analysis-control small{color:var(--muted,#aeb8b3);font-size:.82rem}
  .local-waveform-analysis-control{display:grid;gap:.55rem;margin:.75rem 0 .65rem 2.15rem;padding:.7rem .8rem;border:1px solid var(--line,#35413c);border-radius:8px;background:color-mix(in srgb,var(--surface,#1a211e) 72%,transparent)}
  .local-waveform-analysis-copy{display:grid;gap:.25rem;max-width:60rem}
  .local-waveform-analysis-copy strong{font-size:.9rem}
  .local-waveform-analysis-copy small,.local-waveform-analysis-action small{color:var(--muted,#aeb8b3);font-size:.82rem;line-height:1.45}
  .local-waveform-analysis-action{display:flex;align-items:center;gap:.65rem;flex-wrap:wrap}
  .local-waveform-analysis-action button{min-height:2rem}
</style>
