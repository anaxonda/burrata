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
  constructor(private invoke: Invoke, private uuid: () => string = () => crypto.randomUUID()) {}
  cancel(): void {
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
    if (key === this.lastKey) return;
    this.cancel(); this.lastKey = key;
    const requestId = this.uuid(); this.active = requestId;
    // The backend strips catalog hints and uses the playback-specific search key.
    // Results stay in that cache; no candidate/source is attached to the queue.
    void this.invoke("warm_playback_candidate_search", { track: next, requestId }).catch(() => {}).finally(() => {
      if (this.active === requestId) this.active = null;
    });
  }
}
