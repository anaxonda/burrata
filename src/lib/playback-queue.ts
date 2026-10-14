import type { Track } from "./types";

export type RepeatMode = "off" | "all" | "one";

export interface PlaybackQueueSnapshot {
  tracks: Track[];
  currentIndex: number;
  repeat: RepeatMode;
}

/** A small, UI-independent queue. Indexes refer to the current `tracks` order. */
export class PlaybackQueue {
  private items: Track[] = [];
  private index = -1;
  private repeatMode: RepeatMode = "off";

  get tracks(): readonly Track[] { return this.items; }
  get current(): Track | null { return this.items[this.index] ?? null; }
  get currentIndex(): number { return this.index; }
  get repeat(): RepeatMode { return this.repeatMode; }

  /** Replace the queue with the displayed list and optionally select its starting row. */
  setTracks(tracks: readonly Track[], currentIndex = tracks.length ? 0 : -1): Track | null {
    this.items = [...tracks];
    const requested = Number.isFinite(currentIndex) ? Math.trunc(currentIndex) : 0;
    this.index = this.items.length ? Math.max(0, Math.min(requested, this.items.length - 1)) : -1;
    return this.current;
  }

  setRepeat(mode: RepeatMode): void { this.repeatMode = mode; }

  /** Insert a recording directly after the current item. */
  playNext(track: Track): void {
    if (this.index < 0) {
      this.items.unshift(track);
      this.index = 0;
      return;
    }
    this.items.splice(this.index + 1, 0, track);
  }

  remove(trackId: string): Track | null {
    const at = this.items.findIndex((track) => track.id === trackId);
    return at < 0 ? null : this.removeAt(at);
  }

  removeAt(at: number): Track | null {
    if (!Number.isInteger(at) || at < 0 || at >= this.items.length) return null;
    const [removed] = this.items.splice(at, 1);
    if (!this.items.length) this.index = -1;
    else if (at < this.index) this.index--;
    else if (at === this.index) this.index = Math.min(at, this.items.length - 1);
    return removed ?? null;
  }

  /** Move a row to a new position, keeping the same recording selected. */
  reorder(from: number, to: number): boolean {
    if (!Number.isInteger(from) || !Number.isInteger(to) || from < 0 || to < 0 || from >= this.items.length || to >= this.items.length) return false;
    if (from === to) return true;
    const selected = this.current;
    const [item] = this.items.splice(from, 1);
    if (!item) return false;
    this.items.splice(to, 0, item);
    this.index = selected ? this.items.findIndex((track) => track === selected) : -1;
    return true;
  }

  /** Shuffle upcoming rows, leaving the current recording in place. */
  shuffle(random: () => number = Math.random): void {
    const start = Math.max(0, this.index + 1);
    for (let i = this.items.length - 1; i > start; i--) {
      const j = start + Math.floor(Math.max(0, Math.min(0.999999999, random())) * (i - start + 1));
      [this.items[i], this.items[j]] = [this.items[j]!, this.items[i]!];
    }
  }

  next(): Track | null {
    if (!this.items.length || this.index < 0) return null;
    if (this.repeatMode === "one") return this.current;
    if (this.index + 1 < this.items.length) return this.items[++this.index] ?? null;
    if (this.repeatMode === "all") { this.index = 0; return this.current; }
    return null;
  }

  /** Move past one unavailable recording, stopping after the caller's retry allowance. */
  nextAfterFailedStart(skipped: number, maximumSkips: number): Track | null {
    if (!Number.isInteger(skipped) || !Number.isInteger(maximumSkips) || skipped < 0 || maximumSkips < 0 || skipped >= maximumSkips) return null;
    return this.next();
  }

  previous(): Track | null {
    if (!this.items.length || this.index < 0) return null;
    if (this.index > 0) return this.items[--this.index] ?? null;
    if (this.repeatMode === "all") { this.index = this.items.length - 1; return this.current; }
    return this.current;
  }

  snapshot(): PlaybackQueueSnapshot {
    return { tracks: [...this.items], currentIndex: this.index, repeat: this.repeatMode };
  }

  restore(snapshot: PlaybackQueueSnapshot): Track | null {
    this.repeatMode = ["off", "all", "one"].includes(snapshot.repeat) ? snapshot.repeat : "off";
    return this.setTracks(Array.isArray(snapshot.tracks) ? snapshot.tracks : [], snapshot.currentIndex);
  }
}

const STORAGE_KEY = "burrata.playback-queue.v1";

/** Browser persistence is best-effort; private mode, quota, or disabled storage are harmless. */
export function savePlaybackQueue(queue: PlaybackQueue, storage?: Storage): boolean {
  try {
    const target = storage ?? (typeof localStorage === "undefined" ? undefined : localStorage);
    if (!target) return false;
    target.setItem(STORAGE_KEY, JSON.stringify(queue.snapshot()));
    return true;
  } catch { return false; }
}

export function loadPlaybackQueue(storage?: Storage): PlaybackQueue | null {
  try {
    const target = storage ?? (typeof localStorage === "undefined" ? undefined : localStorage);
    const raw = target?.getItem(STORAGE_KEY);
    if (!raw) return null;
    const value: unknown = JSON.parse(raw);
    if (!value || typeof value !== "object" || !Array.isArray((value as PlaybackQueueSnapshot).tracks)) return null;
    const queue = new PlaybackQueue();
    queue.restore(value as PlaybackQueueSnapshot);
    return queue;
  } catch { return null; }
}
