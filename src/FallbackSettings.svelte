<script lang="ts">
  let { label, enabled, providers, primary, sources, onchange }: {
    label: string; enabled: boolean; providers: string[]; primary: string;
    sources: { id: string; name: string }[];
    onchange: (enabled: boolean, sources: string[]) => void;
  } = $props();
  const primaryName = $derived(sources.find(source=>source.id===primary)?.name ?? primary);
  const choices = $derived(sources.filter(source => source.id !== primary));
  const ordered = $derived(providers.filter((id, index, all) => choices.some(source => source.id === id) && all.indexOf(id) === index));
  const actionName = $derived(label.toLowerCase() === "download" ? "download" : "playback");
  function setSource(index: number, value: string) {
    const next = [...ordered];
    const duplicate = next.indexOf(value);
    if (value && duplicate >= 0 && duplicate !== index) next[duplicate] = next[index] ?? "";
    next[index] = value;
    onchange(enabled, next.filter(Boolean));
  }
</script>
<div class="fallback-settings">
  <label class="check"><input type="checkbox" checked={enabled} aria-label={`Try another source if ${actionName} fails`} onchange={event => onchange(event.currentTarget.checked, ordered)} /><span>Try another source if {actionName} fails</span></label>
  <small class="fallback-chain"><strong>First choice:</strong> {primaryName}{#if enabled && ordered.length}<span aria-hidden="true"> → </span>{#each ordered as id, index}{#if index}<span aria-hidden="true"> → </span>{/if}{sources.find(source=>source.id===id)?.name}{/each}{:else if !enabled}<span class="fallback-only"> · no alternatives</span>{/if}</small>
  <small class="fallback-help">{enabled ? "Burrata tries these sources in order until it finds the same recording." : "Only the first choice is used."}</small>
  {#each choices as _, index}
    <label><span>{index===0 ? "Next" : index===1 ? "Then" : "Last"}</span><select aria-label={`${label} fallback ${index + 1}`} disabled={!enabled} value={ordered[index] ?? ""} onchange={event => setSource(index, event.currentTarget.value)}>
      <option value="">None</option>{#each choices as source}<option value={source.id}>{source.name}</option>{/each}
    </select></label>
  {/each}
</div>
<style>
  .fallback-settings { border-top:1px solid var(--line); padding:14px 0 0; margin-top:16px; }
  .fallback-settings .check { margin:0 0 8px; }
  small { display:block; line-height:1.5; color:var(--muted); }
  label:not(.check) { flex-direction:row; align-items:center; gap:10px; margin:10px 0 0; }
  .fallback-chain { color:var(--text); font-size:11px; }
  .fallback-chain span { color:var(--muted); margin:0 3px; }
  .fallback-help { margin-top:5px; font-size:10px; }
  label:not(.check) > span { width:30px; font-size:10px; }
  select { flex:1; min-width:0; }
</style>
