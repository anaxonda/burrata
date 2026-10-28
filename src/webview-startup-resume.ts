import { hasWebviewSeekLanded } from "./webview-seek-completion-watchdog";

export type StartupResumeProgress = {
  target: number;
  startPosition: number;
  currentTime: number;
  duration: number;
  elapsedMs: number;
  readyState: number;
  seeking: boolean;
  paused: boolean;
  playbackRate: number;
};

export type StartupResumeHistoryGate<TNode = unknown, TSource = unknown> = {
  node: TNode;
  source: TSource;
  trackId: string;
  verified: boolean;
};

/** History must wait until the exact muted startup-resume owner is verified. */
export function startupResumeBlocksHistory<TNode, TSource>(
  gate: StartupResumeHistoryGate<TNode, TSource> | null | undefined,
  node: TNode | null | undefined,
  source: TSource | null | undefined,
  trackId: string,
): boolean {
  return Boolean(gate && gate.node === node && gate.source === source &&
    gate.trackId === trackId && !gate.verified);
}

/** Startup only succeeds after the resumed clock advances from the saved point. */
export function hasWebviewStartupResumeProgress(input: StartupResumeProgress): boolean {
  if (!Number.isFinite(input.target) || input.target <= 0 ||
      !Number.isFinite(input.startPosition) || !Number.isFinite(input.currentTime) ||
      !Number.isFinite(input.elapsedMs) ||
      !Number.isFinite(input.playbackRate) || input.elapsedMs < 0 ||
      input.readyState < 2 || input.seeking || input.paused ||
      input.currentTime - Math.max(input.startPosition, input.target) < 0.25) return false;
  if (Number.isFinite(input.duration) && input.duration > 0 && input.target > input.duration + 2) return false;
  return hasWebviewSeekLanded({
    target: input.target,
    currentTime: input.currentTime,
    duration: input.duration,
    elapsedMs: input.elapsedMs,
    playing: true,
    playbackRate: input.playbackRate,
  });
}
