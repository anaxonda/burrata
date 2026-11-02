<script lang="ts">
  import { safeArtworkUrls } from './lib/artwork-url';
  let { coverUrl, urls = [], fallback = '▤' }: { coverUrl?: string | null; urls?: string[]; fallback?: string } = $props();
  let failed = $state<string[]>([]);
  const signature = $derived(JSON.stringify([coverUrl, urls]));
  $effect(() => { signature; failed = []; });
  const cover = $derived(safeArtworkUrls([coverUrl], 1).find(url => !failed.includes(url)));
  const available = $derived(safeArtworkUrls(urls, 4).filter(url => !failed.includes(url)));
  const images = $derived(cover ? [cover] : available.length === 4 ? available : available.slice(0, 1));
</script>
<span class="playlist-artwork" class:collage={images.length === 4} aria-hidden="true">
  {#if images.length}{#each images as url (url)}<img src={url} alt="" width="34" height="34" loading="lazy" decoding="async" onerror={() => { failed = [...failed, url]; }} />{/each}{:else}<span class="fallback">{fallback}</span>{/if}
</span>
<style>
  .playlist-artwork{display:grid;place-items:center;width:100%;height:100%;overflow:hidden;border-radius:inherit;background:var(--raised);flex:none}.playlist-artwork.collage{grid-template-columns:repeat(2,minmax(0,1fr));grid-template-rows:repeat(2,minmax(0,1fr))}.playlist-artwork img{display:block;width:100%;height:100%;object-fit:cover;min-width:0;min-height:0}.fallback{color:inherit;font-size:inherit}
</style>
