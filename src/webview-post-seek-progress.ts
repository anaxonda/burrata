export type PostSeekProgressOwner = {
  node: HTMLAudioElement;
  sourceGeneration: string | number | null;
  seekSerial: number;
  queueOccurrenceId: string;
};

type Check = {
  owner: PostSeekProgressOwner;
  startTime: number;
  startedAt: number;
  isCurrent: () => boolean;
  onExpired: () => void;
  timer: ReturnType<typeof setTimeout>;
};

/** Verify that a playing seek actually resumes its media clock after landing. */
export class WebviewPostSeekProgress {
  private check: Check | null = null;

  begin(input: { owner: PostSeekProgressOwner; isCurrent: () => boolean; onExpired: () => void; timeoutMs?: number }): void {
    this.cancel();
    const node = input.owner.node;
    if (node.paused || node.seeking || node.ended || node.error || !Number.isFinite(node.currentTime)) return;
    const timeoutMs = input.timeoutMs ?? 5_000;
    if (!Number.isFinite(timeoutMs) || timeoutMs <= 0) return;
    const check: Check = { owner: input.owner, startTime: node.currentTime, startedAt: performance.now(),
      isCurrent: input.isCurrent, onExpired: input.onExpired, timer: 0 as unknown as ReturnType<typeof setTimeout> };
    check.timer = setTimeout(() => {
      if (this.check !== check) return;
      this.check = null;
      if (!check.isCurrent() || node.paused || node.seeking || node.ended || node.error || !Number.isFinite(node.currentTime)) return;
      const elapsed = (performance.now() - check.startedAt) / 1000;
      const delta = node.currentTime - check.startTime;
      // Require real forward progress, allowing timer clamping and slow rates.
      if (delta >= Math.min(0.25, Math.max(0.025, elapsed * 0.05))) return;
      check.onExpired();
    }, timeoutMs);
    this.check = check;
  }

  cancel(): void {
    if (this.check) clearTimeout(this.check.timer);
    this.check = null;
  }
}
