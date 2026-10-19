import type { Track } from "./types";
import type { PlaybackQueueSnapshot } from "./playback-queue";

type Invoke = (command: string, args: Record<string, unknown>) => Promise<unknown>;
export type UpcomingPlayback = {
  webview: boolean; playing: boolean; generation: number; session: string;
  currentTrackId: string; queue: PlaybackQueueSnapshot; pinnedTrackIds?: readonly string[];
};
/** One search-only request per actual upcoming occurrence; never resolves audio. */
export class WebviewNextCandidatePrefetch {
  private active: string | null = null;
  private lastKey = "";
  private attempts = 0;
  private succeeded = false;
  private eligibleKey: string | null = null;
  private retryTimer: ReturnType<typeof setTimeout> | null = null;
  constructor(private invoke: Invoke, private uuid: () => string = () => crypto.randomUUID()) {}
  cancel(): void {
    this.eligibleKey = null;
    if (this.retryTimer !== null) clearTimeout(this.retryTimer);
    this.retryTimer = null;
    const requestId = this.active;
    this.active = null;
    if (requestId) void this.invoke("cancel_remote_read", { requestId }).catch(() => {});
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
