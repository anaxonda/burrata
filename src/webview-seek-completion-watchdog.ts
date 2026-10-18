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
