import type { Track } from "./types";
import { sortTracks, type SortDirection, type TrackSortField } from "./track-sort";

export type RepeatMode = "off" | "all" | "one";

export interface PlaybackQueueEntry {
  entryId: string;
  track: Track;
}

export interface PlaybackQueueSnapshot {
  tracks: Track[];
  /** Missing on legacy snapshots. Every in-memory queue snapshot includes validated IDs. */
  entryIds?: string[];
  currentIndex: number;
  repeat: RepeatMode;
}

let fallbackEntrySequence = 0;
const ENTRY_ID_MAX_LENGTH = 128;

function validEntryId(value: unknown): value is string {
  return typeof value === "string"
    && value.length > 0
    && value.length <= ENTRY_ID_MAX_LENGTH
    && !/[\u0000-\u001f\u007f]/.test(value);
}

function validEntryIds(value: unknown, length: number): value is string[] {
  if (!Array.isArray(value) || value.length !== length) return false;
  const ids = value as unknown[];
  return ids.every(validEntryId) && new Set(ids).size === length;
}

function newEntryId(used: Set<string>): string {
  for (let attempt = 0; attempt < 8; attempt++) {
    let candidate = "";
    try { candidate = globalThis.crypto?.randomUUID?.() ?? ""; } catch { /* Older WebViews may not expose crypto here. */ }
    if (!candidate) {
      fallbackEntrySequence++;
      candidate = `queue-entry-${Date.now().toString(36)}-${fallbackEntrySequence.toString(36)}-${Math.random().toString(36).slice(2)}`;
    }
    if (validEntryId(candidate) && !used.has(candidate)) {
      used.add(candidate);
      return candidate;
    }
  }
  throw new Error("Could not generate a unique queue occurrence ID");
}

function makeEntries(tracks: readonly Track[], entryIds?: readonly string[], alreadyUsed: readonly string[] = []): PlaybackQueueEntry[] {
  const accepted = entryIds && validEntryIds(entryIds, tracks.length) ? entryIds : null;
  if (accepted) return tracks.map((track, index) => ({ entryId: accepted[index]!, track }));
  const used = new Set(alreadyUsed);
  return tracks.map((track) => ({ entryId: newEntryId(used), track }));
}

/** A small UI-independent queue. Indexes refer to occurrence order, never recording identity. */
export class PlaybackQueue {
  private items: PlaybackQueueEntry[] = [];
  private index = -1;
  private repeatMode: RepeatMode = "off";

  get entries(): readonly PlaybackQueueEntry[] { return this.items; }
  get entryIds(): readonly string[] { return this.items.map((entry) => entry.entryId); }
  /** Backwards-compatible projection used by playback and UI callers. */
  get tracks(): readonly Track[] { return this.items.map((entry) => entry.track); }
  get current(): Track | null { return this.items[this.index]?.track ?? null; }
  get currentEntry(): PlaybackQueueEntry | null { return this.items[this.index] ?? null; }
  get currentIndex(): number { return this.index; }
  get repeat(): RepeatMode { return this.repeatMode; }

  /** Replace the queue with displayed occurrences and optionally select its starting row. */
  setTracks(tracks: readonly Track[], currentIndex = tracks.length ? 0 : -1, entryIds?: readonly string[]): Track | null {
    this.items = makeEntries(tracks, entryIds);
    const requested = Number.isFinite(currentIndex) ? Math.trunc(currentIndex) : 0;
    this.index = this.items.length ? Math.max(0, Math.min(requested, this.items.length - 1)) : -1;
    return this.current;
  }

  /** Replace row payloads after enrichment without changing occurrence identity or order. */
  updateTracks(tracks: readonly Track[]): boolean {
    if (tracks.length !== this.items.length) return false;
    this.items = this.items.map((entry, index) => ({ entryId: entry.entryId, track: tracks[index]! }));
    return true;
  }

  setRepeat(mode: RepeatMode): void { this.repeatMode = mode; }

  appendTracks(tracks: readonly Track[]): readonly PlaybackQueueEntry[] {
    if (!tracks.length) return [];
    const added = makeEntries(tracks, undefined, this.items.map((entry) => entry.entryId));
    this.items.push(...added);
    return added;
  }

  appendNextTracks(tracks: readonly Track[]): readonly PlaybackQueueEntry[] {
    if (!tracks.length) return [];
    const added = makeEntries(tracks, undefined, this.items.map((entry) => entry.entryId));
    if (this.index < 0) {
      this.items.unshift(...added);
      this.index = 0;
    } else {
      this.items.splice(this.index + 1, 0, ...added);
    }
    return added;
  }

  /** Compatibility wrapper; each call creates a new occurrence even for the same object. */
  playNext(track: Track): void { this.appendNextTracks([track]); }

  remove(trackId: string): Track | null {
    const at = this.items.findIndex((entry) => entry.track.id === trackId);
    return at < 0 ? null : this.removeAt(at);
  }

  removeAt(at: number): Track | null {
    if (!Number.isInteger(at) || at < 0 || at >= this.items.length) return null;
    const [removed] = this.items.splice(at, 1);
    if (!this.items.length) this.index = -1;
    else if (at < this.index) this.index--;
    else if (at === this.index) this.index = Math.min(at, this.items.length - 1);
    return removed?.track ?? null;
  }

  /** Move an occurrence to a new position, keeping that exact occurrence selected. */
  reorder(from: number, to: number): boolean {
    if (!Number.isInteger(from) || !Number.isInteger(to) || from < 0 || to < 0 || from >= this.items.length || to >= this.items.length) return false;
    if (from === to) return true;
    const selectedIndex = this.index;
    const [item] = this.items.splice(from, 1);
    if (!item) return false;
    this.items.splice(to, 0, item);
    if (selectedIndex === from) this.index = to;
    else if (from < selectedIndex && to >= selectedIndex) this.index = selectedIndex - 1;
    else if (from > selectedIndex && to <= selectedIndex) this.index = selectedIndex + 1;
    else this.index = selectedIndex;
    return true;
  }

  /** Shuffle upcoming occurrences, leaving the current occurrence in place. */
  shuffle(random: () => number = Math.random): void {
    const start = Math.max(0, this.index + 1);
    for (let i = this.items.length - 1; i > start; i--) {
      const j = start + Math.floor(Math.max(0, Math.min(0.999999999, random())) * (i - start + 1));
      [this.items[i], this.items[j]] = [this.items[j]!, this.items[i]!];
    }
  }

  /** Sort only upcoming tracks; wrapper references carry their original IDs through stable sorting. */
  sortUpcoming(field: TrackSortField, direction: SortDirection = "asc"): void {
    const start = Math.max(0, this.index + 1);
    const upcoming = this.items.slice(start).map((entry) => ({ entry, sortTrack: { ...entry.track } as Track }));
    const entryBySortTrack = new Map(upcoming.map(({ entry, sortTrack }) => [sortTrack, entry] as const));
    const sortedTracks = sortTracks(upcoming.map(({ sortTrack }) => sortTrack), field, direction);
    const sortedEntries = sortedTracks.map((track) => entryBySortTrack.get(track)!);
    this.items.splice(start, sortedEntries.length, ...sortedEntries);
  }

  next(): Track | null {
    if (!this.items.length || this.index < 0) return null;
    if (this.repeatMode === "one") return this.current;
    if (this.index + 1 < this.items.length) return this.items[++this.index]?.track ?? null;
    if (this.repeatMode === "all") { this.index = 0; return this.current; }
    return null;
  }

  /** Failed starts traverse the finite supplied order once, regardless of repeat. */
  nextAfterFailedStart(visitedEntryIds: ReadonlySet<string>): Track | null {
    const next = this.index + 1;
    if (this.index < 0 || next >= this.items.length || visitedEntryIds.has(this.items[next]!.entryId)) return null;
    this.index = next;
    return this.current;
  }

  previous(): Track | null {
    if (!this.items.length || this.index < 0) return null;
    if (this.index > 0) return this.items[--this.index]?.track ?? null;
    if (this.repeatMode === "all") { this.index = this.items.length - 1; return this.current; }
    return this.current;
  }

  snapshot(): PlaybackQueueSnapshot {
    return { tracks: this.items.map((entry) => entry.track), entryIds: this.items.map((entry) => entry.entryId), currentIndex: this.index, repeat: this.repeatMode };
  }

  restore(snapshot: PlaybackQueueSnapshot): Track | null {
    this.repeatMode = ["off", "all", "one"].includes(snapshot.repeat) ? snapshot.repeat : "off";
    return this.setTracks(Array.isArray(snapshot.tracks) ? snapshot.tracks : [], snapshot.currentIndex, snapshot.entryIds);
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

/** Index-only occurrence planner: duplicates remain distinct and selected order stays stable. */
export function planQueueSelectionMove(length: number, selected: readonly number[], target: number): { moves: [number, number][]; selected: number[] } {
  const indices = [...new Set(selected)].filter(index => Number.isInteger(index) && index >= 0 && index < length).sort((a,b)=>a-b);
  if (!indices.length || !Number.isInteger(target)) return {moves:[],selected:indices};
  const chosen = new Set(indices);
  const remaining = Array.from({length},(_,index)=>index).filter(index=>!chosen.has(index));
  const insertion = Math.max(0,Math.min(target,remaining.length));
  const desired = [...remaining.slice(0,insertion),...indices,...remaining.slice(insertion)];
  const current = Array.from({length},(_,index)=>index);
  const moves: [number,number][]=[];
  desired.forEach((value,to)=>{ const from=current.indexOf(value); if(from!==to){moves.push([from,to]);current.splice(to,0,current.splice(from,1)[0]!);} });
  return {moves,selected:indices.map((_,index)=>insertion+index)};
}
