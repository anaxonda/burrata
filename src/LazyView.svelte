<script lang="ts">
  import { untrack } from "svelte";
  import type { Component } from "svelte";

  type Props = {
    loader: () => Promise<Component<any>>;
    componentProps?: Record<string, unknown>;
    label?: string;
    onInstance?: (instance: unknown) => void;
  };

  let { loader, componentProps = {}, label = "view", onInstance }: Props = $props();
  let attempt = $state(0);
  let loadRequest = $state<Promise<Component<any>> | null>(null);
  let instance = $state<unknown>(null);

  $effect(() => {
    untrack(() => { if (!loadRequest) loadRequest = loader(); });
    const boundInstance = instance;
    untrack(() => onInstance?.(boundInstance));
  });

  function retry() {
    attempt += 1;
    loadRequest = loader();
  }
</script>

{#key attempt}
  {#if loadRequest}
    {#await loadRequest}
      <div class="lazy-view-status" role="status" aria-live="polite">Loading {label}…</div>
    {:then View}
      <View bind:this={instance} {...componentProps} />
    {:catch}
      <div class="lazy-view-error" role="alert">
        <span>Could not load {label}.</span>
        <button type="button" onclick={retry}>Retry</button>
      </div>
    {/await}
  {:else}
    <div class="lazy-view-status" role="status" aria-live="polite">Loading {label}…</div>
  {/if}
{/key}

<style>
  .lazy-view-status, .lazy-view-error { padding: 1rem; }
  .lazy-view-error { display: flex; align-items: center; gap: .75rem; }
</style>
