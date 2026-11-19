<script lang="ts">
  import type { ArtworkUpgradeCandidate } from "./lib/fullscreen-artwork";
  let { urls = [], upgradeCandidates = [], ambient = false }: { urls?: (string | null | undefined)[]; upgradeCandidates?: ArtworkUpgradeCandidate[]; ambient?: boolean } = $props();
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

  const shortSide = (width: number, height: number) => Math.min(width, height);

  function tryUpgrade(node: HTMLImageElement, originalSize: number) {
    if (upgradeAttempted || !upgradeCandidates.length) return;
    const target = targetPixelWidth(node);
    if (originalSize >= target) return;
    upgradeAttempted = true;
    const token = upgradeGeneration;
    const improvements = [...new Map(upgradeCandidates
      .filter(candidate => shortSide(candidate.width, candidate.height) > originalSize
        && candidate.url !== node.currentSrc && candidate.url !== node.src)
      .map(candidate => [candidate.url, candidate])).values()];
    const sizeOf = (candidate: ArtworkUpgradeCandidate) => shortSide(candidate.width, candidate.height);
    // Preserve the provider's image preference for candidates that can fill the
    // view. If those fail, seek the largest improvement over the current image.
    const candidates = [
      ...improvements.filter(candidate => sizeOf(candidate) >= target),
      ...improvements.filter(candidate => sizeOf(candidate) < target)
        .sort((left, right) => sizeOf(right) - sizeOf(left)),
    ];
    let best: { url: string; size: number } | null = null;
    const loadNext = (candidateIndex: number) => {
      if (token !== upgradeGeneration) return;
      if (candidateIndex >= candidates.length) {
        if (best) upgradedSource = best.url;
        return;
      }
      const probe = new Image();
      const candidate = candidates[candidateIndex]!;
      probe.onload = () => {
        if (token !== upgradeGeneration) return;
        const size = shortSide(probe.naturalWidth, probe.naturalHeight);
        if (size >= target) {
          upgradedSource = candidate.url;
          return;
        }
        if (size > (best?.size ?? originalSize)) best = { url: candidate.url, size };
        loadNext(candidateIndex + 1);
      };
      probe.onerror = () => loadNext(candidateIndex + 1);
      probe.src = candidate.url;
    };
    loadNext(0);
  }

  function onImageLoad(event: Event) {
    const node = event.currentTarget as HTMLImageElement;
    tryUpgrade(node, shortSide(node.naturalWidth, node.naturalHeight));
  }

  function onImageError(event: Event) {
    const node = event.currentTarget as HTMLImageElement;
    if (upgradedSource) {
      upgradedSource = null;
      return;
    }
    index += 1;
    if (index >= sources.length) tryUpgrade(node, 0);
  }
</script>

<span class="image-frame" aria-hidden="true">
  {#if source}<img src={source} alt="" loading="lazy" onload={onImageLoad} onerror={onImageError} />
    {#if ambient}<img class="artwork-ambient" src={source} alt="" loading="lazy" decoding="async" />{/if}
  {:else}<span class="placeholder">♫</span>{/if}
</span>

<style>
  .image-frame{display:grid;place-items:center;width:100%;height:100%;overflow:hidden;border-radius:inherit;background:var(--raised)}
  img:not(.artwork-ambient){width:100%;height:100%;object-fit:cover}.placeholder{color:var(--muted)}
</style>
