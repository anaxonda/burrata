<script lang="ts">
  import InfoPopover from "./InfoPopover.svelte";
  import { DEFAULT_FLEXIBLE_MATCHING, type FlexibleMatching } from "./lib/types";
  let { value = DEFAULT_FLEXIBLE_MATCHING, noticePending = false, onchange, onNoticeDismiss }: {
    value?: FlexibleMatching; noticePending?: boolean;
    onchange: (value: FlexibleMatching) => void; onNoticeDismiss?: () => void;
  } = $props();
  type Key = Exclude<keyof FlexibleMatching, "version">;
  const options: { key: Key; label: string; help: string }[] = [
    { key: "allowOtherReleases", label: "Different releases of the same recording", help: "For example, the same recording on a compilation or reissue." },
    { key: "allowOtherPerformances", label: "Other performances by the same artist", help: "For example, a live performance instead of the studio recording." },
    { key: "allowOtherMixes", label: "Other remixes or edits", help: "For example, a radio edit or another remix." },
    { key: "allowOtherArtists", label: "The same song performed by other artists", help: "For example, a cover by another artist." },
    { key: "allowDifferentLengths", label: "Allow much longer or shorter versions", help: "Burrata prefers the closest length. For individual songs it excludes albums, medleys and previews; this option permits verified full alternatives with a larger length difference." },
  ];
</script>
<div class="flexible-options">
  <p>Choose which alternatives Flexible matching can use for playback and downloads. An enabled option can replace a specifically named version.</p>
  {#if noticePending}<div class="notice" role="status">Flexible matching now has separate choices. Only different releases are enabled until you choose more alternatives. {#if onNoticeDismiss}<button type="button" onclick={onNoticeDismiss}>Got it</button>{/if}</div>{/if}
  {#each options as option}
    <div class="option"><label><input type="checkbox" checked={value[option.key]} onchange={event => onchange({...value, [option.key]:event.currentTarget.checked})} /><span>{option.label}</span></label><InfoPopover label={'About ' + option.label}>{option.help}</InfoPopover></div>
  {/each}
</div>
<style>
  .flexible-options{display:grid;gap:.5rem;margin-top:.4rem}.flexible-options p,.notice{margin:0;color:var(--muted);font-size:.82rem;line-height:1.45}.option{display:flex;align-items:center;gap:.45rem}.option label{display:flex;flex-direction:row;align-items:center;gap:.5rem;margin:0;color:var(--text);font-size:.85rem}.option input{width:auto;flex:none;margin:0}.notice button{margin-left:.4rem}
</style>
