<script lang="ts">
  import InfoPopover from "./InfoPopover.svelte";
  import { alternativeSourceNotice } from "./lib/alternative-source-notice";
  let { matching, requestedTitle, requestedArtist }: { matching: unknown; requestedTitle?: string | null; requestedArtist?: string | null } = $props();
  const notice = $derived(alternativeSourceNotice(matching, requestedTitle, requestedArtist));
</script>
{#if notice}<span class="alternative-source"><span>{notice.label}{notice.performer ? ` · ${notice.performer}` : ""}</span><InfoPopover label="About this alternative" placement="top">Requested: {notice.requested}. Playing: {notice.playing}. Your library details are unchanged.</InfoPopover></span>{/if}
<style>
  .alternative-source{display:inline-flex;align-items:center;gap:.3rem;max-width:100%;color:var(--muted,#a3a8ae);font-size:.75rem}.alternative-source > span{overflow:hidden;text-overflow:ellipsis;white-space:nowrap}
</style>
