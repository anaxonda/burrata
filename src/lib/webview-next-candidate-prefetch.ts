import type { Track } from "./types";
import type { PlaybackQueueSnapshot } from "./playback-queue";

type Invoke = (command: string, args: Record<string, unknown>) => Promise<unknown>;
export type UpcomingPlayback = {
  webview: boolean; playing: boolean; generation: number; session: string;
  currentTrackId: string; queue: PlaybackQueueSnapshot; pinnedTrackIds?: readonly string[];
  /** Seconds until the current WebView item ends, if its handoff is known. */
  remainingSeconds?: number | null;
};
/** Search early, then resolve a validated source near the actual handoff. */
export class WebviewNextCandidatePrefetch {
  private active: string | null = null;
  private preparing: string | null = null;
  private lastKey = "";
  private attempts = 0;
  private succeeded = false;
  private eligibleKey: string | null = null;
  private retryTimer: ReturnType<typeof setTimeout> | null = null;
  private preparationTimer: ReturnType<typeof setTimeout> | null = null;
  private preparationStarted = false;
  constructor(private invoke: Invoke, private uuid: () => string = () => crypto.randomUUID()) {}
  cancel(): void {
    this.eligibleKey = null;
    if (this.retryTimer !== null) clearTimeout(this.retryTimer);
    this.retryTimer = null;
    if (this.preparationTimer !== null) clearTimeout(this.preparationTimer);
    this.preparationTimer = null;
    const requestId = this.active;
    this.active = null;
    if (requestId) void this.invoke("cancel_remote_read", { requestId }).catch(() => {});
    const preparationId = this.preparing;
    this.preparing = null;
    this.preparationStarted = false;
    if (preparationId) void this.invoke("cancel_remote_read", { requestId: preparationId }).catch(() => {});
  }
  started(value: UpcomingPlayback): void {
    const { queue } = value;
    const ids = queue.entryIds;
    const current = queue.tracks[queue.currentIndex];
    if (!value.webview || !value.playing || !value.session || !current || current.id !== value.currentTrackId || current.provider === "radio"
      || !ids || ids.length !== queue.tracks.length || new Set(ids).size !== ids.length || queue.repeat === "one") {
      this.cancel(); return;
    }
    const index = queue.currentIndex + 1 < queue.tracks.length ? queue.currentIndex + 1 : queue.repeat === "all" ? 0 : -1;
    const next: Track | undefined = queue.tracks[index];
    if (!next || index === queue.currentIndex || ["radio", "youtube", "soundcloud"].includes(next.provider) || value.pinnedTrackIds?.includes(next.id) || next.localPath || next.playbackRange) {
      this.cancel(); return;
    }
    const key = JSON.stringify([value.generation, value.session, ids[queue.currentIndex], ids[index], next]);
    if (key !== this.lastKey) {
      this.cancel();
      this.lastKey = key;
      this.attempts = 0;
      this.succeeded = false;
    }
    this.eligibleKey = key;
    this.startRequest(key, next);
    this.schedulePlaybackPreparation(key, next, value.remainingSeconds);
  }
  private schedulePlaybackPreparation(key: string, next: Track, remainingSeconds?: number | null): void {
    if (this.preparationStarted || this.eligibleKey !== key) return;
    if (this.preparationTimer !== null) clearTimeout(this.preparationTimer);
    this.preparationTimer = null;
    if (remainingSeconds === null || remainingSeconds === undefined || !Number.isFinite(remainingSeconds)) return;
    const remaining = Math.max(0, remainingSeconds);
    const waitMs = Math.max(0, (remaining - 45) * 1000);
    this.preparationTimer = setTimeout(() => {
      this.preparationTimer = null;
      this.startPlaybackPreparation(key, next, Math.min(45_000, Math.max(50, remaining <= 45 ? remaining * 1000 : 45_000)));
    }, waitMs);
  }
  private startPlaybackPreparation(key: string, next: Track, handoffInMs: number): void {
    if (this.eligibleKey !== key || this.preparationStarted || this.preparing) return;
    this.preparationStarted = true;
    const requestId = this.uuid();
    this.preparing = requestId;
    void this.invoke("prepare_upcoming_playback_source", { track: next, requestId, handoffInMs }).then(() => {
      if (this.preparing === requestId) this.preparing = null;
    }, () => {
      if (this.preparing === requestId) this.preparing = null;
    });
  }
  private startRequest(key: string, next: Track): void {
    // One bounded retry covers a cancelled/failed search. Repeated playing
    // events cannot start overlapping work or bypass the failure backoff.
    if (this.eligibleKey !== key || this.active || this.retryTimer !== null || this.succeeded || this.attempts >= 2) return;
    const requestId = this.uuid();
    this.active = requestId;
    this.attempts++;
    void this.invoke("warm_playback_candidate_search", { track: next, requestId }).then(() => {
      if (this.active !== requestId) return;
      this.active = null;
      this.succeeded = true;
    }, () => {
      if (this.active !== requestId) return;
      this.active = null;
      if (this.attempts >= 2 || this.eligibleKey !== key) return;
      this.retryTimer = setTimeout(() => {
        this.retryTimer = null;
        this.startRequest(key, next);
      }, 1000);
    });
  }
}
