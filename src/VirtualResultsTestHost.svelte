<script lang="ts">
  import VirtualResults from "./VirtualResults.svelte";
  let { count = 500 }: { count?: number } = $props();
  const items = $derived(Array.from({ length: count }, (_, index) => index));
  let selected = $state("");
</script>

<div class="queue" style="height: 300px; overflow: auto">
  <VirtualResults {items} itemKey={(item) => String(item)} estimateHeight={() => 40} scrollKey="test-downloads" regionKey="download-jobs" scrollRootSelector=".queue" ariaLabel="Download job rows">
    {#snippet children(item, index)}<article data-row={index} data-job-id={`job-${item}`} style="height:40px">Job {item}<button aria-label={`Retry job-${item}`} onclick={() => selected = `job-${item}`}>Retry</button></article>{/snippet}
  </VirtualResults>
  <output data-selected>{selected}</output>
</div>
