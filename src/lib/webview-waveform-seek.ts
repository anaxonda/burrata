export type WebviewWaveformSeekDrain = {
  node: HTMLAudioElement;
  sourceGeneration: number | null;
  seekSerial: number;
  targetPositionSeconds: number;
};

export type WebviewWaveformWindow = {
  startPositionSeconds: number;
  endPositionSeconds: number;
};

/**
 * After a seek, wait until the analyser's entire sampled interval begins at
 * the requested position. That requires media-clock advancement by at least
 * the analyser window length; elapsed wall time alone cannot prove its ring
 * buffer contains post-seek audio.
 */
export function isFreshWebviewWaveformWindowAfterSeek(
  node: HTMLAudioElement,
  sourceGeneration: number | null,
  seekSerial: number,
  window: WebviewWaveformWindow,
  drain: WebviewWaveformSeekDrain | null,
): boolean {
  if (node.seeking || node.readyState < 2) return false;
  if (!drain || drain.node !== node || drain.sourceGeneration !== sourceGeneration || drain.seekSerial !== seekSerial) return true;
  return Number.isFinite(window.startPositionSeconds) &&
    Number.isFinite(window.endPositionSeconds) &&
    window.startPositionSeconds >= drain.targetPositionSeconds - 0.001 &&
    window.endPositionSeconds > drain.targetPositionSeconds;
}
