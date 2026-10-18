export type VirtualWindow = { start: number; end: number; top: number; bottom: number };

type SavedRegion = { width: number; heights: Map<string, number> };
type SavedView = { scrollTop: number; regions: Map<string, SavedRegion> };
const savedViews = new Map<string, SavedView>();
type ScrollRootState = { activeKey: string | null; attachments: number };
const scrollRoots = new WeakMap<HTMLElement, ScrollRootState>();
export type VirtualScrollRootLease = { root: HTMLElement; active: boolean };
function scrollRootState(root: HTMLElement): ScrollRootState {
  const state = scrollRoots.get(root) ?? { activeKey: null, attachments: 0 };
  scrollRoots.set(root, state);
  return state;
}
function viewState(key: string): SavedView {
  const state = savedViews.get(key) ?? { scrollTop: 0, regions: new Map<string, SavedRegion>() };
  savedViews.delete(key);
  savedViews.set(key, state);
  while (savedViews.size > 32) {
    const oldest = savedViews.keys().next().value;
    if (oldest === undefined) break;
    savedViews.delete(oldest);
  }
  return state;
}
export function getSavedVirtualScroll(key: string): number { return viewState(key).scrollTop; }
export function setSavedVirtualScroll(key: string, position: number): void { viewState(key).scrollTop = position; }
export function getSavedVirtualHeights(viewKey: string, regionKey: string, width: number): Map<string, number> {
  const region = viewState(viewKey).regions.get(regionKey);
  return region && Math.abs(region.width - width) <= 32 ? new Map(region.heights) : new Map();
}
export function setSavedVirtualHeights(viewKey: string, regionKey: string, width: number, measured: ReadonlyMap<string, number>): void {
  const view = viewState(viewKey);
  const prior = view.regions.get(regionKey);
  const heights = prior && Math.abs(prior.width - width) <= 32 ? prior.heights : new Map<string, number>();
  for (const [key, height] of measured) {
    heights.delete(key);
    heights.set(key, height);
  }
  while (heights.size > 2_000) {
    const oldest = heights.keys().next().value;
    if (oldest === undefined) break;
    heights.delete(oldest);
  }
  view.regions.set(regionKey, { width, heights });
}
export function activateVirtualScroll(root: HTMLElement, key: string, currentPosition: number): number | null {
  const state = scrollRootState(root);
  if (state.activeKey === key) return null;
  if (state.activeKey) setSavedVirtualScroll(state.activeKey, currentPosition);
  state.activeKey = key;
  return getSavedVirtualScroll(key);
}
export function acquireVirtualScrollRoot(root: HTMLElement): VirtualScrollRootLease {
  scrollRootState(root).attachments++;
  return { root, active: true };
}
export function releaseVirtualScrollRoot(lease: VirtualScrollRootLease, key: string, position: number): void {
  if (!lease.active) return;
  lease.active = false;
  const root = lease.root;
  const state = scrollRoots.get(root);
  if (!state || state.attachments === 0) return;
  if (state.activeKey === key) setSavedVirtualScroll(key, position);
  state.attachments--;
  if (state.attachments === 0) state.activeKey = null;
}
export function scrollAnchorDelta(index: number, anchorIndex: number, before: number, after: number): number {
  return index < anchorIndex ? after - before : 0;
}
export async function afterVirtualTick(waitForTick: () => Promise<unknown>, isDisposed: () => boolean, attach: () => void): Promise<void> {
  await waitForTick();
  if (!isDisposed()) attach();
}

export function prefixHeights(count: number, heights: readonly number[], estimate: (index: number) => number): number[] {
  const prefix = new Array<number>(count + 1);
  prefix[0] = 0;
  for (let i = 0; i < count; i++) prefix[i + 1] = prefix[i]! + Math.max(1, heights[i] ?? estimate(i));
  return prefix;
}

function rowAt(values: readonly number[], value: number): number {
  const count = Math.max(0, values.length - 1);
  if (!count) return 0;
  let low = 0, high = count;
  while (low < high) {
    const middle = (low + high) >>> 1;
    if (values[middle + 1]! <= value) low = middle + 1;
    else high = middle;
  }
  return Math.min(low, count - 1);
}

export function virtualWindow(prefix: readonly number[], offset: number, viewportHeight: number, overscan = 5): VirtualWindow {
  const count = Math.max(0, prefix.length - 1);
  if (!count) return { start: 0, end: 0, top: 0, bottom: 0 };
  const boundedOffset = Math.max(0, Math.min(offset, Math.max(0, prefix[count]! - Math.max(1, viewportHeight))));
  const first = rowAt(prefix, boundedOffset);
  const viewportEnd = boundedOffset + Math.max(1, viewportHeight);
  const last = rowAt(prefix, Math.max(boundedOffset, viewportEnd - 0.5));
  const start = Math.max(0, first - overscan);
  const end = Math.min(count, last + overscan + 1);
  return { start, end, top: prefix[start]!, bottom: prefix[count]! - prefix[end]! };
}
