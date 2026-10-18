import type { Track } from "./types";
import { trackIdentity } from "./track-identity";

export type QueueRadioFetch = (seed: Track, signal: AbortSignal) => Promise<readonly Track[]>;
export type QueueRadioRepeat = "off" | "all" | "one";

interface PendingRequest {
  generation: number;
  seedKey: string;
  queueKey: string;
  abort: AbortController;
  promise: Promise<void>;
  recommendations: readonly Track[] | null;
  excludedKeys: readonly string[];
}

/**
 * Holds one recommendation request for the final queued track. It never starts
 * playback itself; callers may take recommendations only after a natural queue
 * end with repeat off. Generation checks discard IPC results that cannot abort.
 */
export class QueueRadioController {
  private currentGeneration = 0;
  private pending: PendingRequest | null = null;

  constructor(private readonly fetchRecommendations: QueueRadioFetch) {}

  get generation(): number { return this.currentGeneration; }
  get isPending(): boolean { return this.pending !== null; }

  prepare(seed: Track, queue: readonly Track[], recent: readonly Track[]): void {
    this.cancel();
    if (!isFinalSeed(seed, queue) || !seed.id.trim()) return;

    const abort = new AbortController();
    const request: PendingRequest = {
      generation: ++this.currentGeneration,
      seedKey: queueRadioSeedKey(seed),
      queueKey: queueKey(queue),
      abort,
      promise: Promise.resolve(),
      recommendations: null,
      excludedKeys: [...new Set([...queue, ...recent].flatMap(trackKeys))],
    };
    request.promise = Promise.resolve()
      .then(() => this.fetchRecommendations(seed, abort.signal))
      .then((tracks) => {
        if (this.pending === request && !abort.signal.aborted) {
          request.recommendations = tracks;
        }
      })
      .catch(() => {
        // A recommendation lookup is optional. Playback of the queue remains intact.
      });
    this.pending = request;
  }

  /** Cancel a pending lookup and invalidate any result that arrives afterward. */
  cancel(): void {
    this.currentGeneration++;
    this.pending?.abort.abort();
    this.pending = null;
  }

  /**
   * Await a ready lookup at natural end, then remove anything now in the queue,
   * recently played, or duplicated in the recommendation response.
   */
  async take(
    seed: Track,
    queue: readonly Track[],
    recent: readonly Track[],
    endedNaturally: boolean,
    repeat: QueueRadioRepeat,
  ): Promise<Track[]> {
    if (!endedNaturally || repeat !== "off" || !isFinalSeed(seed, queue)) {
      this.cancel();
      return [];
    }
    const request = this.pending;
    if (!request) return [];
    if (request.seedKey !== queueRadioSeedKey(seed) || request.queueKey !== queueKey(queue)) {
      this.cancel();
      return [];
    }

    await waitForRequestOrAbort(request);
    if (this.pending !== request || request.abort.signal.aborted) return [];

    const excluded = new Set<string>();
    for (const key of request.excludedKeys) excluded.add(key);
    for (const track of [...queue, ...recent]) {
      for (const key of trackKeys(track)) excluded.add(key);
    }
    const seen = new Set(excluded);
    const result = (request.recommendations ?? []).filter((track) => {
      const keys = trackKeys(track);
      if (!keys.length || keys.some((key) => seen.has(key))) return false;
      for (const key of keys) seen.add(key);
      return true;
    });

    this.pending = null;
    this.currentGeneration++;
    return [...result];
  }
}

function isFinalSeed(seed: Track, queue: readonly Track[]): boolean {
  return queue.length > 0 && identityKey(queue[queue.length - 1]!) === identityKey(seed);
}

function queueKey(queue: readonly Track[]): string {
  return JSON.stringify(queue.map(queueRadioSeedKey));
}

function identityKey(track: Track): string {
  return trackIdentity(track);
}

/** Recommendation results are tied to the playable source as well as the row ID. */
/** Stable source-aware identity for a radio recommendation seed or queue row. */
export function queueRadioSeedKey(track: Track): string {
  const provenance = asRecord(track.provenance);
  const candidate = asRecord(provenance?.audioCandidate);
  return JSON.stringify([
    identityKey(track),
    track.provider,
    track.providerId,
    track.sourceUrl,
    track.localPath ?? null,
    track.assetId ?? null,
    candidate?.provider ?? null,
    candidate?.id ?? null,
    candidate?.url ?? null,
  ]);
}

function trackKeys(track: Track): string[] {
  const keys = new Set<string>();
  keys.add(`identity:${trackIdentity(track)}`);
  const isChapterRange = Boolean(track.playbackRange);
  if (!isChapterRange && track.id.trim()) keys.add(`id:${track.id.trim()}`);
  if (!isChapterRange && track.provider.trim() && track.providerId.trim()) {
    keys.add(`provider:${track.provider.trim().toLowerCase()}:${track.providerId.trim()}`);
  }
  const provenance = asRecord(track.provenance);
  const candidate = asRecord(provenance?.audioCandidate);
  if (!isChapterRange && typeof candidate?.id === "string" && candidate.id.trim()) {
    keys.add(`id:${candidate.id.trim()}`);
  }
  if (!isChapterRange && typeof candidate?.provider === "string" && typeof candidate.id === "string") {
    keys.add(`provider:${candidate.provider.trim().toLowerCase()}:${candidate.id.trim()}`);
  }
  for (const url of isChapterRange ? [] : [track.sourceUrl, candidate?.url]) {
    if (typeof url !== "string") continue;
    const normalized = sourceIdentity(url);
    if (normalized) keys.add(`url:${normalized}`);
  }
  return [...keys];
}

function asRecord(value: unknown): Record<string, unknown> | null {
  return value && typeof value === "object" && !Array.isArray(value)
    ? value as Record<string, unknown>
    : null;
}

function sourceIdentity(raw: string): string | null {
  try {
    const url = new URL(raw);
    if (url.protocol !== "http:" && url.protocol !== "https:") return null;
    const host = url.hostname.toLowerCase().replace(/^www\./, "");
    if (host === "youtube.com" || host === "music.youtube.com") {
      const videoId = url.searchParams.get("v");
      return videoId ? `youtube:${videoId}` : null;
    }
    if (host === "youtu.be") {
      const videoId = url.pathname.split("/").filter(Boolean)[0];
      return videoId ? `youtube:${videoId}` : null;
    }
    return `${host}${url.pathname.replace(/\/$/, "")}`;
  } catch {
    return null;
  }
}

async function waitForRequestOrAbort(request: PendingRequest): Promise<void> {
  const { signal } = request.abort;
  if (signal.aborted) return;
  let onAbort: (() => void) | undefined;
  const aborted = new Promise<void>((resolve) => {
    onAbort = resolve;
    signal.addEventListener("abort", onAbort, { once: true });
  });
  try {
    await Promise.race([request.promise, aborted]);
  } finally {
    if (onAbort) signal.removeEventListener("abort", onAbort);
  }
}
