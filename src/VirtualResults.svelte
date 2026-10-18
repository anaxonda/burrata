<script lang="ts" generics="T">
  import { onMount, onDestroy, tick } from "svelte";
  import type { Snippet } from "svelte";
  import { acquireVirtualScrollRoot, activateVirtualScroll, afterVirtualTick, getSavedVirtualHeights, prefixHeights, releaseVirtualScrollRoot, scrollAnchorDelta, setSavedVirtualHeights, virtualWindow } from "./lib/virtual-results";
  import type { VirtualScrollRootLease } from "./lib/virtual-results";

  let { items, itemKey, estimateHeight, scrollKey, regionKey, scrollRootSelector = ".search-panel", ariaLabel = "Search result rows", children }: {
    items: T[];
    itemKey: (item: T) => string;
    estimateHeight: (item: T) => number;
    scrollKey: string;
    regionKey: string;
    scrollRootSelector?: string;
    ariaLabel?: string;
    children: Snippet<[T, number]>;
  } = $props();

  let host: HTMLDivElement;
  let panel: HTMLElement | null = null;
  let activeScrollKey = $state("");
  let sizes = new Map<string, number>();
  let cacheSizes = new Map<string, number>();
  let measuredWidth = 0;
  let disposed = false;
  let listeningPanel: HTMLElement | null = null;
  let scrollRootLease: VirtualScrollRootLease | null = null;
  let scrollTop = $state(0);
  let viewportHeight = $state(720);
  let measurementRevision = $state(0);
  let raf = 0;
  const resizeObserver = typeof ResizeObserver === "undefined" ? null : new ResizeObserver(entries => {
    if (disposed) return;
    let anchor = currentWindow.start;
    let scrollDelta = 0;
    let changed = false;
    for (const entry of entries) {
      const element = entry.target as HTMLElement;
      if (element === host || element === panel) {
        const width = host.clientWidth || panel?.clientWidth || 0;
        if (measuredWidth && width && Math.abs(width - measuredWidth) > 32) {
          sizes.clear();
          cacheSizes.clear();
          measuredWidth = width;
          measurementRevision++;
        }
        scheduleUpdate();
        continue;
      }
      const key = element.dataset.virtualKey;
      if (!key) continue;
      const next = entry.borderBoxSize?.[0]?.blockSize ?? entry.contentRect.height;
      const prior = sizes.get(key);
      if (prior === undefined || Math.abs(prior - next) > 0.5) {
        const index = indices.get(key);
        if (index !== undefined && prior !== undefined) scrollDelta += scrollAnchorDelta(index, anchor, prior, next);
        if (index !== undefined) {
          sizes.set(key, next);
          cacheSizes.delete(key);
          cacheSizes.set(key, next);
          while (cacheSizes.size > 2_000) cacheSizes.delete(cacheSizes.keys().next().value!);
        }
        changed = true;
      }
    }
    if (scrollDelta && panel) panel.scrollTop += scrollDelta;
    if (changed) {
      measurementRevision++;
      if (measuredWidth) setSavedVirtualHeights(scrollKey, regionKey, measuredWidth, cacheSizes);
      scheduleUpdate();
    }
  });
  const heights = $derived.by(() => { measurementRevision; return items.map(item => sizes.get(itemKey(item)) ?? estimateHeight(item)); });
  const indices = $derived(new Map(items.map((item, index) => [itemKey(item), index])));
  const prefix = $derived(prefixHeights(items.length, heights, () => 64));
  let currentWindow = $derived(virtualWindow(prefix, localOffset(), viewportHeight, 6));
  const visibleItems = $derived(items.slice(currentWindow.start, currentWindow.end));

  function localOffset(): number {
    if (!panel || !host) return 0;
    const panelRect = panel.getBoundingClientRect();
    const hostRect = host.getBoundingClientRect();
    const contentOffset = hostRect.top - panelRect.top + panel.scrollTop;
    return Math.max(0, scrollTop - contentOffset);
  }
  function scheduleUpdate() {
    if (raf && typeof cancelAnimationFrame === "function") cancelAnimationFrame(raf);
    const update = () => {
      raf = 0;
      if (panel) { scrollTop = panel.scrollTop; viewportHeight = panel.clientHeight || 720; }
    };
    if (typeof requestAnimationFrame === "function") raf = requestAnimationFrame(update);
    else update();
  }
  function onScroll() { scheduleUpdate(); }
  function focusItem(index: number, last: boolean) {
    if (!panel) return;
    const panelRect = panel.getBoundingClientRect();
    const hostRect = host.getBoundingClientRect();
    const contentOffset = hostRect.top - panelRect.top + panel.scrollTop;
    panel.scrollTop = Math.max(0, contentOffset + prefix[index]!);
    scrollTop = panel.scrollTop;
    viewportHeight = panel.clientHeight || 720;
    panel.dispatchEvent(new Event("scroll"));
    void tick().then(() => {
      const item = host.querySelector<HTMLElement>(`[data-virtual-index="${index}"]`);
      const controls = item ? Array.from(item.querySelectorAll<HTMLElement>("button:not(:disabled),a[href],input:not(:disabled),select:not(:disabled),textarea:not(:disabled),summary,[tabindex]:not([tabindex='-1'])")).filter(control => !control.closest("details:not([open])") || control.matches("summary")) : [];
      (last ? controls[controls.length - 1] : controls[0])?.focus();
    });
  }
  function onKeydown(event: KeyboardEvent) {
    if (event.key !== "Tab" || !panel || !host || !(event.target instanceof Node) || !host.contains(event.target)) return;
    const focusableSelector = "button:not(:disabled),a[href],input:not(:disabled),select:not(:disabled),textarea:not(:disabled),summary,[tabindex]:not([tabindex='-1'])";
    const controls = Array.from(host.querySelectorAll<HTMLElement>(focusableSelector)).filter(control => !control.closest("details:not([open])") || control.matches("summary"));
    if (!controls.length) return;
    if (event.shiftKey && event.target === controls[0] && currentWindow.start > 0) {
      event.preventDefault();
      focusItem(currentWindow.start - 1, true);
    } else if (!event.shiftKey && event.target === controls[controls.length - 1] && currentWindow.end < items.length) {
      event.preventDefault();
      focusItem(currentWindow.end, false);
    }
  }
  function register(element: HTMLElement, key: string) {
    element.dataset.virtualKey = key;
    resizeObserver?.observe(element);
    return { destroy() { resizeObserver?.unobserve(element); } };
  }

  $effect(() => {
    const currentItems = items;
    const live = new Set(currentItems.map(itemKey));
    for (const key of sizes.keys()) if (!live.has(key)) sizes.delete(key);
    for (const key of cacheSizes.keys()) if (!live.has(key)) cacheSizes.delete(key);
  });
  $effect(() => {
    const nextKey = scrollKey;
    if (!panel || !activeScrollKey || nextKey === activeScrollKey) return;
    if (measuredWidth) setSavedVirtualHeights(activeScrollKey, regionKey, measuredWidth, cacheSizes);
    const nextPosition = activateVirtualScroll(panel, nextKey, panel.scrollTop);
    activeScrollKey = nextKey;
    sizes = getSavedVirtualHeights(nextKey, regionKey, measuredWidth);
    cacheSizes = new Map(sizes);
    measurementRevision++;
    if (nextPosition !== null) {
      panel.scrollTop = nextPosition;
      scheduleUpdate();
    }
  });

  function cleanup() {
    if (listeningPanel) {
      if (scrollRootLease) releaseVirtualScrollRoot(scrollRootLease, activeScrollKey, listeningPanel.scrollTop);
      scrollRootLease = null;
      if (measuredWidth) setSavedVirtualHeights(scrollKey, regionKey, measuredWidth, cacheSizes);
      listeningPanel.removeEventListener("scroll", onScroll);
      listeningPanel.removeEventListener("keydown", onKeydown);
      listeningPanel = null;
    }
    resizeObserver?.disconnect();
  }

  onMount(() => {
    disposed = false;
    panel = host.closest<HTMLElement>(scrollRootSelector);
    if (!panel) return cleanup;
    const mountedPanel = panel;
    void afterVirtualTick(() => tick(), () => disposed, () => {
      if (panel !== mountedPanel || !host.isConnected) return;
      measuredWidth = host.clientWidth || mountedPanel.clientWidth || 0;
      sizes = getSavedVirtualHeights(scrollKey, regionKey, measuredWidth);
      cacheSizes = new Map(sizes);
      measurementRevision++;
      setSavedVirtualHeights(scrollKey, regionKey, measuredWidth, cacheSizes);
      activeScrollKey = scrollKey;
      scrollRootLease = acquireVirtualScrollRoot(mountedPanel);
      const restored = activateVirtualScroll(mountedPanel, activeScrollKey, mountedPanel.scrollTop);
      if (restored !== null) mountedPanel.scrollTop = restored;
      scrollTop = mountedPanel.scrollTop;
      viewportHeight = mountedPanel.clientHeight || 720;
      listeningPanel = mountedPanel;
      mountedPanel.addEventListener("scroll", onScroll, { passive: true });
      mountedPanel.addEventListener("keydown", onKeydown);
      resizeObserver?.observe(host);
      resizeObserver?.observe(mountedPanel);
    });
    return () => { disposed = true; cleanup(); };
  });
  onDestroy(() => {
    disposed = true;
    cleanup();
    if (raf && typeof cancelAnimationFrame === "function") cancelAnimationFrame(raf);
  });
</script>

<div class="virtual-results" role="group" aria-label={ariaLabel} tabindex="-1" bind:this={host}>
  <div class="virtual-spacer" aria-hidden="true" style={`height:${currentWindow.top}px`}></div>
  {#each visibleItems as item, localIndex (itemKey(item))}
    {@const index = currentWindow.start + localIndex}
    <div class="virtual-result-item" data-virtual-index={index} use:register={itemKey(item)}>{@render children(item, index)}</div>
  {/each}
  <div class="virtual-spacer" aria-hidden="true" style={`height:${currentWindow.bottom}px`}></div>
</div>

<style>
  .virtual-results{min-width:0}.virtual-spacer{height:0;pointer-events:none}.virtual-result-item{min-width:0}
</style>
