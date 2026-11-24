import { safeArtworkUrl } from "./artwork-url";

export type ArtworkLookup = {
  key: string;
  load: () => Promise<string | null>;
  validate?: (value: unknown) => string | null;
};

const cache = new Map<string, string>();
const inFlight = new Map<string, Promise<string | null>>();
const waiting = new Set<() => void>();
const maxConcurrent = 2;

export function observeArtwork(node: HTMLElement, request: ArtworkLookup & { apply: (url: string) => void }) {
  let active = true;
  const apply = (url: string | null) => { if (active && url) request.apply(url); };

  function start() {
    if (!active) return;
    const cached = cache.get(request.key);
    if (cached) {
      apply(cached);
      return;
    }
    const existing = inFlight.get(request.key);
    if (existing) {
      void existing.then(apply);
      return;
    }
    const pending = Promise.resolve().then(request.load).then(request.validate ?? safeArtworkUrl).catch(() => null);
    inFlight.set(request.key, pending);
    void pending.then(url => {
      inFlight.delete(request.key);
      if (url) {
        cache.set(request.key, url);
        if (cache.size > 400) cache.delete(cache.keys().next().value!);
      }
      apply(url);
      for (const next of waiting) {
        if (inFlight.size >= maxConcurrent) break;
        waiting.delete(next);
        next();
      }
    });
  }

  const schedule = () => {
    if (inFlight.size < maxConcurrent || cache.has(request.key) || inFlight.has(request.key)) start();
    else waiting.add(start);
  };
  const observer = typeof IntersectionObserver === "function" ? new IntersectionObserver(entries => {
    if (!entries.some(entry => entry.isIntersecting)) return;
    observer?.disconnect();
    schedule();
  }, { rootMargin: "120px" }) : null;
  if (observer) observer.observe(node);
  else schedule();
  return { destroy() { active = false; observer?.disconnect(); waiting.delete(start); } };
}
