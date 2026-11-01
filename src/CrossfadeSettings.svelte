<script lang="ts">
  import InfoPopover from "./InfoPopover.svelte";
  let { enabled = false, seconds = 5, onchange }: { enabled?: boolean; seconds?: number; onchange: (enabled: boolean, seconds: number) => void } = $props();
</script>
<div class="crossfade">
  <div class="heading"><label><input type="checkbox" checked={enabled} onchange={event => onchange(event.currentTarget.checked, seconds)} /><span>Crossfade songs</span></label><InfoPopover label="About crossfade">Blend songs during natural transitions, including albums. Changes apply to the next transition.</InfoPopover></div>
  {#if enabled}<label class="duration">Fade duration <select aria-label="Crossfade duration" value={seconds} onchange={event => onchange(enabled, Number(event.currentTarget.value))}>{#each Array.from({length:12}, (_, i) => i + 1) as n}<option value={n}>{n} {n === 1 ? "second" : "seconds"}</option>{/each}</select></label>{/if}
</div>
<style>
  .crossfade{display:grid;gap:.5rem;padding:.85rem 0;border-top:1px solid var(--line,#454a50)}.heading{display:flex;align-items:center;gap:.45rem}.heading label{display:flex;flex-direction:row;align-items:center;gap:.5rem;margin:0;color:var(--text);font-size:.86rem}.heading input{width:auto;flex:none;margin:0}.duration{display:grid;gap:.35rem;margin:0;font-size:.86rem;max-width:15rem}
</style>
