<script lang="ts">
  import InfoPopover from "./InfoPopover.svelte";
  import { DEFAULT_FLEXIBLE_MATCHING, type FlexibleMatching } from "./lib/types";
  let { value = DEFAULT_FLEXIBLE_MATCHING, noticePending = false, onchange, onNoticeDismiss }: {
    value?: FlexibleMatching; noticePending?: boolean;
    onchange: (value: FlexibleMatching) => void; onNoticeDismiss?: () => void;
  } = $props();
  type ToggleKey = "allowOtherReleases" | "allowOtherPerformances" | "allowOtherMixes" | "allowOtherArtists" | "allowDifferentLengths";
  type DurationKey = "maxShorterBySeconds" | "maxLongerBySeconds";
  const options: { key: ToggleKey; label: string; help: string }[] = [
    { key: "allowOtherReleases", label: "Different releases of the same recording", help: "For example, the same recording on a compilation or reissue." },
    { key: "allowOtherPerformances", label: "Other performances by the same artist", help: "For example, a live performance instead of the studio recording." },
    { key: "allowOtherMixes", label: "Other remixes or edits", help: "For example, a radio edit or another remix." },
    { key: "allowOtherArtists", label: "The same song performed by other artists", help: "For example, a cover by another artist." },
    { key: "allowDifferentLengths", label: "Allow different durations for identified recordings", help: "These caps include Flexible matching's usual duration tolerance. If a saved track has no duration, enabling limits can prevent an automatic match. Recording identity checks still apply." },
  ];
  const normalized = $derived({
    ...DEFAULT_FLEXIBLE_MATCHING,
    ...value,
    maxShorterBySeconds: value.maxShorterBySeconds ?? DEFAULT_FLEXIBLE_MATCHING.maxShorterBySeconds!,
    maxLongerBySeconds: value.maxLongerBySeconds ?? DEFAULT_FLEXIBLE_MATCHING.maxLongerBySeconds!,
  });
  function updateToggle(key: ToggleKey, checked: boolean) {
    onchange({ ...normalized, [key]: checked });
  }
  function updateDurationLimit(key: DurationKey, raw: string) {
    const parsed = Number(raw);
    if (!Number.isFinite(parsed)) return;
    const seconds = Math.min(3600, Math.max(0, Math.round(parsed)));
    onchange({ ...normalized, [key]: seconds });
  }
</script>
<div class="flexible-options">
  <p>Choose which alternatives Flexible matching can use for playback and downloads. An enabled option can replace a specifically named version.</p>
  {#if noticePending}<div class="notice" role="status">Flexible matching now has separate choices. Only different releases are enabled until you choose more alternatives. {#if onNoticeDismiss}<button type="button" onclick={onNoticeDismiss}>Got it</button>{/if}</div>{/if}
  {#each options as option}
    <div class="option"><label><input type="checkbox" checked={normalized[option.key]} onchange={event => updateToggle(option.key, event.currentTarget.checked)} /><span>{option.label}</span></label><InfoPopover label={'About ' + option.label}>{option.help}</InfoPopover></div>
    {#if option.key === "allowDifferentLengths"}
      <fieldset class="duration-limits" disabled={!normalized.allowDifferentLengths}>
        <legend>Total difference from requested duration</legend>
        <label>Shorter by up to<input aria-label="Maximum shorter duration difference (seconds)" type="number" min="0" max="3600" step="1" value={normalized.maxShorterBySeconds} onchange={event => updateDurationLimit("maxShorterBySeconds", event.currentTarget.value)} />seconds</label>
        <label>Longer by up to<input aria-label="Maximum longer duration difference (seconds)" type="number" min="0" max="3600" step="1" value={normalized.maxLongerBySeconds} onchange={event => updateDurationLimit("maxLongerBySeconds", event.currentTarget.value)} />seconds</label>
        <p>0 seconds requires an exact known duration. Limits are measured against the requested track.</p>
      </fieldset>
    {/if}
  {/each}
</div>
<style>
  .flexible-options{display:grid;gap:.5rem;margin-top:.4rem}.flexible-options p,.notice{margin:0;color:var(--muted);font-size:.82rem;line-height:1.45}.option{display:flex;align-items:center;gap:.45rem}.option label{display:flex;flex-direction:row;align-items:center;gap:.5rem;margin:0;color:var(--text);font-size:.85rem}.option input{width:auto;flex:none;margin:0}.notice button{margin-left:.4rem}.duration-limits{display:flex;flex-wrap:wrap;gap:.55rem 1rem;margin:0 0 .25rem 1.5rem;padding:.5rem .65rem;border:1px solid var(--line);border-radius:6px}.duration-limits legend{padding:0 .2rem;color:var(--muted);font-size:.78rem}.duration-limits label{display:flex;align-items:center;gap:.4rem;color:var(--text);font-size:.8rem}.duration-limits input{width:5.5rem;padding:.25rem .4rem}.duration-limits p{flex-basis:100%;font-size:.75rem}.duration-limits:disabled{opacity:.55}
</style>
