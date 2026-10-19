export type WebviewStallOwner = {
  node: HTMLAudioElement;
  sourceGeneration: string | number | null;
  seekSerial: number;
  queueOccurrenceId: string | null;
};

type ActiveStall = {
  owner: WebviewStallOwner;
  mediaTime: number;
  startedAtMs: number;
  timer: ReturnType<typeof setTimeout>;
  isCurrent: () => boolean;
  onExpired: () => void;
  timeoutMs: number;
};

/** Owner-scoped recovery after finite or live media stops making playable progress. */
export class WebviewStallWatchdog {
  private active: ActiveStall | null = null;

  reset(): void {
    if (this.active) clearTimeout(this.active.timer);
    this.active = null;
  }

  arm(input: {
    owner: WebviewStallOwner;
    currentNode: HTMLAudioElement | null;
    isCurrent: () => boolean;
    mediaTime: number;
    duration: number;
    paused: boolean;
    seeking: boolean;
    ended: boolean;
    hasError: boolean;
    live: boolean;
    onExpired: () => void;
    timeoutMs?: number;
  }): void {
    const { owner } = input;
    if (owner.node !== input.currentNode || input.paused || input.seeking || input.ended || input.hasError ||
        (!input.live && (!Number.isFinite(input.duration) || input.duration <= 0)) ||
        !Number.isFinite(input.mediaTime)) return;
    if (this.active && sameOwner(this.active.owner, owner)) return;
    this.reset();
    this.schedule(owner, input.mediaTime, input.isCurrent, input.onExpired, input.timeoutMs ?? (input.live ? 5_000 : 20_000));
  }

  observeProgress(owner: WebviewStallOwner, mediaTime: number): void {
    const active = this.active;
    const wallDeltaMs = active ? performance.now() - active.startedAtMs : 0;
    if (active && sameOwner(active.owner, owner) && Number.isFinite(mediaTime) && plausibleProgress(mediaTime - active.mediaTime, wallDeltaMs)) {
      // Real progress postpones recovery, but keep watching in case a brief buffer drains and freezes.
      clearTimeout(active.timer);
      this.schedule(active.owner, mediaTime, active.isCurrent, active.onExpired, active.timeoutMs);
    }
  }

  private schedule(owner: WebviewStallOwner, mediaTime: number, isCurrent: () => boolean, onExpired: () => void, timeoutMs: number): void {
    const active = { owner, mediaTime, startedAtMs: performance.now(), timer: 0 as ReturnType<typeof setTimeout>, isCurrent, onExpired, timeoutMs };
    active.timer = setTimeout(() => {
      if (this.active !== active) return;
      this.active = null;
      if (!isCurrent()) return;
      const currentTime = owner.node.currentTime;
      if (owner.node.paused || owner.node.seeking || owner.node.ended || owner.node.error || !Number.isFinite(currentTime)) return;
      const elapsedMs = performance.now() - active.startedAtMs;
      const mediaDelta = currentTime - mediaTime;
      // Background timer clamping can hide timeupdates. Treat plausible actual clock movement as
      // progress; an implausibly slow trickle still counts as a stalled clock.
      if (plausibleProgress(mediaDelta, elapsedMs) || mediaDelta > elapsedMs / 1000 * 1.5 + 0.1) {
        this.schedule(owner, currentTime, isCurrent, onExpired, timeoutMs);
        return;
      }
      onExpired();
    }, timeoutMs);
    this.active = active;
  }
}

function plausibleProgress(mediaDelta: number, wallDeltaMs: number): boolean {
  if (!Number.isFinite(mediaDelta) || !Number.isFinite(wallDeltaMs) || wallDeltaMs < 100) return false;
  const wallSeconds = wallDeltaMs / 1000;
  return mediaDelta >= Math.max(0.025, wallSeconds * 0.5 - 0.05) && mediaDelta <= wallSeconds * 1.5 + 0.1;
}

function sameOwner(a: WebviewStallOwner, b: WebviewStallOwner): boolean {
  return a.node === b.node && a.sourceGeneration === b.sourceGeneration && a.seekSerial === b.seekSerial &&
    a.queueOccurrenceId === b.queueOccurrenceId;
}
