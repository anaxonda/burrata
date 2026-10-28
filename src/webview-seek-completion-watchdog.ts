export type WebviewSeekCompletionOwner = {
  node: HTMLAudioElement;
  sourceGeneration: string | number | null;
  seekSerial: number;
  queueOccurrenceId: string;
};

type PendingSeek = {
  owner: WebviewSeekCompletionOwner;
  target: number;
  isCurrent: () => boolean;
  onExpired: (target: number) => void;
  timer: ReturnType<typeof setTimeout> | null;
};

/** A bounded, owner-scoped completion deadline for manual finite-source seeks. */
export class WebviewSeekCompletionWatchdog {
  private pending: PendingSeek | null = null;

  begin(input: {
    owner: WebviewSeekCompletionOwner;
    target: number;
    paused: boolean;
    isCurrent: () => boolean;
    onExpired: (target: number) => void;
    timeoutMs?: number;
  }): void {
    this.cancel();
    if (!Number.isFinite(input.target) || input.target < 0) return;
    this.pending = { owner: input.owner, target: input.target, isCurrent: input.isCurrent,
      onExpired: input.onExpired, timer: null };
    if (!input.paused) this.arm(input.timeoutMs ?? 20_000);
  }

  resume(owner: WebviewSeekCompletionOwner, timeoutMs = 20_000): void {
    if (this.pending && this.pending.timer === null && sameOwner(this.pending.owner, owner) && this.pending.isCurrent()) this.arm(timeoutMs);
  }

  pause(owner?: WebviewSeekCompletionOwner): void {
    if (!this.pending || (owner && !sameOwner(this.pending.owner, owner))) return;
    if (this.pending.timer !== null) clearTimeout(this.pending.timer);
    this.pending.timer = null;
  }

  complete(owner: WebviewSeekCompletionOwner): void {
    if (this.pending && sameOwner(this.pending.owner, owner)) this.cancel();
  }

  cancel(): void {
    if (this.pending?.timer !== null && this.pending?.timer !== undefined) clearTimeout(this.pending.timer);
    this.pending = null;
  }

  private arm(timeoutMs: number): void {
    const pending = this.pending;
    if (!pending || !Number.isFinite(timeoutMs) || timeoutMs <= 0) return;
    if (pending.timer !== null) clearTimeout(pending.timer);
    pending.timer = setTimeout(() => {
      if (this.pending !== pending) return;
      pending.timer = null;
      if (!pending.isCurrent()) { this.cancel(); return; }
      this.pending = null;
      pending.onExpired(pending.target);
    }, timeoutMs);
  }
}

function sameOwner(a: WebviewSeekCompletionOwner, b: WebviewSeekCompletionOwner): boolean {
  return a.node === b.node && a.sourceGeneration === b.sourceGeneration &&
    a.seekSerial === b.seekSerial && a.queueOccurrenceId === b.queueOccurrenceId;
}

/** Media may already have advanced when a queued seeked/timeupdate event runs.
 * Bound forward progress by the actual elapsed playback clock instead of
 * comparing a moving playhead only with the original stationary target.
 */
export function hasWebviewSeekLanded(input: {
  target: number; currentTime: number; duration: number; elapsedMs: number;
  playing: boolean; playbackRate: number; browserAcknowledged?: boolean;
}): boolean {
  if (!Number.isFinite(input.target) || input.target < 0 || !Number.isFinite(input.currentTime) ||
      !Number.isFinite(input.elapsedMs) || input.elapsedMs < 0) return false;
  // A current seeked event acknowledges the browser's completed seek. WebKit
  // AAC can land about a second beyond the target before playback advances;
  // use the existing maximum two-second landing bound for that acknowledgment.
  // Ordinary time updates keep the tighter bound and elapsed-playback check.
  const tolerance = input.browserAcknowledged ? 2
    : Math.max(0.75, Math.min(2, Number.isFinite(input.duration) ? input.duration * 0.005 : 0.75));
  const advance = input.playing && Number.isFinite(input.playbackRate) && input.playbackRate > 0
    ? input.elapsedMs / 1000 * input.playbackRate : 0;
  return input.currentTime >= input.target - tolerance && input.currentTime <= input.target + tolerance + advance;
}
