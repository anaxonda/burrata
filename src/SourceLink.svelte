<script lang="ts">
  import { call } from "./lib/api";
  import { trackSource } from "./lib/source-link";
  import type { Track } from "./lib/types";
  let { track, compact = false }: { track: Track; compact?: boolean } = $props();
  const source = $derived(trackSource(track));
  let failure = $state("");
  async function open(event: MouseEvent) {
    event.stopPropagation();
    if (!source) return;
    failure = "";
    try { await call("open_external_url", { url: source.url }); }
    catch (error) { failure = String(error); }
  }
</script>

{#if source}
  <button class="source-link" class:compact title={failure || `Open ${track.title} on ${source.label}`}
    aria-label={`Open ${track.title} on ${source.label}`} onclick={open}>
    {#if !compact}Open on {source.label} {/if}↗
  </button>
  {#if failure && !compact}<small role="alert">{failure}</small>{/if}
{/if}

<style>
  .source-link { color: var(--muted); }
  .compact { padding: 4px 7px; border-color: transparent; font-size: 15px; }
  small { color: var(--danger, #e79a93); }
</style>
