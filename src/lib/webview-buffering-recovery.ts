export type WebviewProgressOwner = {
  node: object;
  sourceGeneration: string | number | null;
  seekSerial: number;
};

type ProgressSample = WebviewProgressOwner & { mediaTime: number; wallTimeMs: number };

/** Clears a UI buffering indicator only after repeated plausible media-clock progress. */
export class WebviewBufferingRecovery {
  private previous: ProgressSample | null = null;
  private confirmedIntervals = 0;

  reset(): void {
    this.previous = null;
    this.confirmedIntervals = 0;
  }

  observe(input: {
    owner: WebviewProgressOwner;
    currentNode: object | null | undefined;
    mediaTime: number;
    wallTimeMs: number;
    paused: boolean;
    seeking: boolean;
    ended: boolean;
    hasError: boolean;
  }): boolean {
    if (input.owner.node !== input.currentNode) return false;
    if (input.paused || input.seeking || input.ended || input.hasError ||
        !Number.isFinite(input.mediaTime) || !Number.isFinite(input.wallTimeMs)) {
      this.reset();
      return false;
    }

    const sample: ProgressSample = { ...input.owner, mediaTime: input.mediaTime, wallTimeMs: input.wallTimeMs };
    const previous = this.previous;
    if (!previous || previous.node !== sample.node || previous.sourceGeneration !== sample.sourceGeneration || previous.seekSerial !== sample.seekSerial) {
      this.previous = sample;
      this.confirmedIntervals = 0;
      return false;
    }

    const wallDeltaMs = sample.wallTimeMs - previous.wallTimeMs;
    const mediaDelta = sample.mediaTime - previous.mediaTime;
    const wallSeconds = wallDeltaMs / 1000;
    const plausible = wallDeltaMs >= 100 && wallDeltaMs <= 2500 &&
      mediaDelta >= Math.max(0.025, wallSeconds * 0.5 - 0.05) &&
      mediaDelta <= wallSeconds * 1.5 + 0.1;
    this.previous = sample;
    if (!plausible) {
      this.confirmedIntervals = 0;
      return false;
    }

    this.confirmedIntervals = Math.min(2, this.confirmedIntervals + 1);
    return this.confirmedIntervals >= 2;
  }
}
