<script lang="ts">
  export type StreamMediaAnalysisActionState = "idle" | "starting" | "running" | "finalizing";
  let { eligible, state = "idle", status = "", onStart, onCancel }: {
    eligible: boolean;
    state?: StreamMediaAnalysisActionState;
    status?: string;
    onStart: () => void;
    onCancel: () => void;
  } = $props();

  const visible = $derived(eligible || state !== "idle");
  const cancelable = $derived(state === "running");
  const busy = $derived(state === "starting" || state === "finalizing");
  const label = $derived(state === "starting"
    ? "Starting faster-than-realtime full stream analysis"
    : state === "running"
      ? "Cancel faster-than-realtime full stream analysis"
      : state === "finalizing"
        ? "Finishing full stream analysis"
        : "Analyze full stream for BPM, key, and waveform");
  const actionText = $derived(state === "starting" ? "Starting…"
    : state === "running" ? "Cancel analysis"
    : state === "finalizing" ? "Finishing…"
    : /incomplete|unavailable|failed|could not/i.test(status) ? "Retry analysis" : "Analyze current stream");
  const tooltip = $derived(status || (state === "idle"
    ? "Analyze the verified stream in one bounded local pass while playback continues"
    : label));
</script>

{#if visible}
  <button
    class="stream-analysis-action"
    type="button"
    aria-label={label}
    aria-busy={busy ? "true" : undefined}
    title={tooltip}
    disabled={busy || (!eligible && !cancelable)}
    onclick={() => cancelable ? onCancel() : state === "idle" && eligible ? onStart() : undefined}
  >
    {#if busy}
      <svg class="stream-analysis-spinner" aria-hidden="true" viewBox="0 0 24 24" fill="none">
        <circle cx="12" cy="12" r="8.5" stroke="currentColor" stroke-opacity=".28" stroke-width="2" />
        <path d="M20.5 12a8.5 8.5 0 0 0-8.5-8.5" stroke="currentColor" stroke-width="2" stroke-linecap="round" />
      </svg>
    {:else if cancelable}
      <svg aria-hidden="true" viewBox="0 0 24 24" fill="none">
        <rect x="6" y="6" width="12" height="12" rx="2" fill="currentColor" />
      </svg>
    {:else}
      <svg aria-hidden="true" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round">
        <path d="M3.5 18.5h17" />
        <path d="M5.5 15v-4M9.8 15V6.5M14.2 15V9M18.5 15v-6" />
        <path d="m16.3 5.2 1.4-1.4 2.5 2.5-1.4 1.4z" />
      </svg>
    {/if}
    <span>{actionText}</span>
  </button>
{/if}

<style>
  button { display:inline-flex; align-items:center; gap:.45rem; padding:.4rem .65rem; min-height:2rem; }
  svg { width:1.1rem; height:1.1rem; flex:none; }
  .stream-analysis-spinner { animation:spin 1s linear infinite; }
  @keyframes spin { to { transform:rotate(360deg); } }
  @media (prefers-reduced-motion:reduce) { .stream-analysis-spinner { animation:none; } }
</style>
