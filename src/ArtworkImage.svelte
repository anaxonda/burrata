<script lang="ts">
  import type { ArtworkUpgradeCandidate } from "./lib/fullscreen-artwork";
  let { urls = [], upgradeCandidates = [] }: { urls?: (string | null | undefined)[]; upgradeCandidates?: ArtworkUpgradeCandidate[] } = $props();
  const sources = $derived([...new Set(urls.filter((url): url is string => Boolean(url)))]);
  const signature = $derived(`${sources.join("\n")}\n--upgrade--\n${upgradeCandidates.map(candidate => candidate.url).join("\n")}`);
  let index = $state(0);
  let upgradedSource = $state<string | null>(null);
  let upgradeAttempted = false;
  let upgradeGeneration = 0;
  $effect(() => { signature; index = 0; upgradedSource = null; upgradeAttempted = false; upgradeGeneration++; });
  const source = $derived(upgradedSource ?? sources[index]);

  function targetPixelWidth(node: HTMLImageElement) {
    const cssWidth = node.parentElement?.clientWidth || 600;
    const density = typeof devicePixelRatio === "number" ? Math.max(1, Math.min(devicePixelRatio, 2)) : 1;
    return Math.min(1200, Math.max(600, Math.ceil(cssWidth * density)));
  }

  function tryUpgrade(node: HTMLImageElement) {
    if (upgradeAttempted || !upgradeCandidates.length || node.naturalWidth >= targetPixelWidth(node)) return;
    upgradeAttempted = true;
    const token = upgradeGeneration;
    const target = targetPixelWidth(node);
    const candidates = [...new Map(upgradeCandidates
      .filter(candidate => candidate.width >= target && candidate.height >= target && candidate.url !== node.currentSrc && candidate.url !== node.src)
      .map(candidate => [candidate.url, candidate])).values()];
    const loadNext = (candidateIndex: number) => {
      if (token !== upgradeGeneration || candidateIndex >= candidates.length) return;
      const probe = new Image();
      const candidate = candidates[candidateIndex]!;
      probe.onload = () => {
        if (token !== upgradeGeneration) return;
        if (probe.naturalWidth >= target) upgradedSource = candidate.url;
        else loadNext(candidateIndex + 1);
      };
      probe.onerror = () => loadNext(candidateIndex + 1);
      probe.src = candidate.url;
    };
    loadNext(0);
  }
</script>

<span class="image-frame" aria-hidden="true">
  {#if source}<img src={source} alt="" loading="lazy" onload={(event) => tryUpgrade(event.currentTarget as HTMLImageElement)} onerror={() => { if (upgradedSource) upgradedSource = null; else index += 1; }} />
  {:else}<span class="placeholder">♫</span>{/if}
</span>

<style>
  .image-frame{display:grid;place-items:center;width:100%;height:100%;overflow:hidden;border-radius:inherit;background:var(--raised)}
  img{width:100%;height:100%;object-fit:cover}.placeholder{color:var(--muted)}
</style>
