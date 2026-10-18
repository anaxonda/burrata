<script lang="ts">
  import { getContext, type Snippet } from "svelte";
  import { SOURCE_OPENER, type SourceOpener } from "./lib/source-opening";
  import { call } from "./lib/api";
  import { trackSource } from "./lib/source-link";
  import type { Track } from "./lib/types";
  let { track, compact = false, trailingArrow = true, onOpened, leadingIcon }: { track: Track; compact?: boolean; trailingArrow?: boolean; onOpened?: () => void; leadingIcon?: Snippet } = $props();
  const opener = getContext<SourceOpener | undefined>(SOURCE_OPENER);
  const source = $derived(trackSource(track));
  let failure = $state("");
  async function open(event: MouseEvent) {
    event.stopPropagation();
    if (!source) return;
    failure = "";
    try { if(opener) await opener(track, source.url); else await call("open_external_url", { url: source.url }); onOpened?.(); }
    catch (error) { failure = String(error); }
  }
</script>

{#if source}
  <button class="source-link" class:compact title={failure || `Open ${track.title} on ${source.label}`}
    aria-label={`Open ${track.title} on ${source.label}`} onclick={open}>
    {@render leadingIcon?.()}
    {#if !compact}Open on {source.label}{/if}{#if trailingArrow} ↗{/if}
  </button>
  {#if failure && !compact}<small role="alert">{failure}</small>{/if}
{/if}

<style>
  .source-link { color: var(--muted); }
  .compact { padding: 4px 7px; border-color: transparent; font-size: 15px; }
  small { color: var(--danger, #e79a93); }
</style>
