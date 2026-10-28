<script lang="ts">
  export type StreamAnalysisActionState = "idle" | "preparing" | "capturing" | "finalizing";

  let { eligible, state = "idle", onStart, onCancel }: {
    eligible: boolean;
    state?: StreamAnalysisActionState;
    onStart: () => void;
    onCancel: () => void;
  } = $props();

  const visible = $derived(eligible || state !== "idle");
  const cancelable = $derived(state === "capturing");
  const busy = $derived(state === "preparing" || state === "finalizing");
  const label = $derived(state === "preparing"
    ? "Preparing whole-track BPM and key analysis"
    : state === "capturing"
      ? "Cancel whole-track BPM and key analysis"
      : state === "finalizing"
        ? "Finalizing whole-track BPM and key analysis"
        : "Analyze this stream for BPM and key");
  const tooltip = $derived(state === "preparing"
    ? "Restarting the supported stream and preparing its full-track capture"
    : state === "capturing"
      ? "Cancel whole-track BPM and key analysis"
      : state === "finalizing"
        ? "Waiting for captured audio analysis to finish"
        : "Analyze stream BPM and key");
</script>

{#if visible}
  <button
    class="stream-analysis-action"
    type="button"
    aria-label={label}
    aria-busy={busy ? "true" : undefined}
    title={tooltip}
    disabled={busy}
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
  </button>
{/if}
