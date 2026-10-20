<script lang="ts">
  let { urls = [] }: { urls?: (string | null | undefined)[] } = $props();
  const sources = $derived([...new Set(urls.filter((url): url is string => Boolean(url)))]);
  const signature = $derived(sources.join("\n"));
  let index = $state(0);
  $effect(() => { signature; index = 0; });
</script>

<span class="image-frame" aria-hidden="true">
  {#if sources[index]}<img src={sources[index]} alt="" loading="lazy" onerror={() => { index += 1; }} />
  {:else}<span class="placeholder">♫</span>{/if}
</span>

<style>
  .image-frame{display:grid;place-items:center;width:100%;height:100%;overflow:hidden;border-radius:inherit;background:var(--raised)}
  img{width:100%;height:100%;object-fit:cover}.placeholder{color:var(--muted)}
</style>
