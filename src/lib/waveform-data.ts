/** Playback waveforms keep unobserved audio visibly empty. These summaries
 * contain only amplitude bounds; they never retain or expose PCM samples.
 */

export type WaveformBin = { min: number; max: number };
export type WaveformSource = "observed-playback" | "local-analysis" | "stream-analysis";
export type WaveformSnapshot = {
  generation: number;
  trackId: string;
  durationSeconds: number;
  bins: Array<WaveformBin | null>;
  source: WaveformSource;
  /** True only when a full-resource analysis completed successfully. */
  complete: boolean;
  reason?: string;
  scope?: "wholeTrack" | "playbackRange";
  resource?: { provider: string; sourceId: string; sourceIdentitySha256: string; bindingKey?: string };
};
export type TimedWaveformSegment = {
  generation: number;
  trackId: string;
  startSeconds: number;
  endSeconds: number;
  min: number;
  max: number;
};

export const DEFAULT_WAVEFORM_BIN_COUNT = 1200;
export const MAX_WAVEFORM_BIN_COUNT = 4096;

function validDuration(durationSeconds: number): boolean {
  return Number.isFinite(durationSeconds) && durationSeconds > 0 && durationSeconds <= 24 * 60 * 60;
}
function validCount(binCount: number): boolean {
  return Number.isSafeInteger(binCount) && binCount > 0 && binCount <= MAX_WAVEFORM_BIN_COUNT;
}
function validAmplitude(min: number, max: number): boolean {
  return Number.isFinite(min) && Number.isFinite(max) && min >= -1 && max <= 1 && min <= max;
}

export function emptyWaveform(
  generation: number,
  trackId: string,
  durationSeconds: number,
  binCount = DEFAULT_WAVEFORM_BIN_COUNT,
  reason?: string,
): WaveformSnapshot {
  if (!Number.isSafeInteger(generation) || generation < 0) throw new Error("Invalid waveform generation.");
  if (typeof trackId !== "string" || !trackId || trackId.length > 512) throw new Error("Invalid waveform track identity.");
  if (!validDuration(durationSeconds)) throw new Error("Invalid waveform duration.");
  if (!validCount(binCount)) throw new Error("Invalid waveform bin count.");
  return { generation, trackId, durationSeconds, bins: Array.from({ length: binCount }, () => null), source: "observed-playback", complete: false, reason };
}

/** Merge one generation-owned observation into the existing bounded bins.
 * A short segment may touch one or two bins; it never fills gaps between
 * separate observations.
 */
export function observeWaveformSegment(snapshot: WaveformSnapshot, segment: TimedWaveformSegment): WaveformSnapshot {
  if (snapshot.source !== "observed-playback" || snapshot.complete) return snapshot;
  if (segment.generation !== snapshot.generation || segment.trackId !== snapshot.trackId) return snapshot;
  if (!Number.isFinite(segment.startSeconds) || !Number.isFinite(segment.endSeconds) ||
      segment.startSeconds < 0 || segment.endSeconds <= segment.startSeconds ||
      segment.startSeconds >= snapshot.durationSeconds || !validAmplitude(segment.min, segment.max)) return snapshot;
  const start = Math.max(0, segment.startSeconds);
  const end = Math.min(snapshot.durationSeconds, segment.endSeconds);
  const count = snapshot.bins.length;
  const first = Math.min(count - 1, Math.floor(start / snapshot.durationSeconds * count));
  const last = Math.min(count - 1, Math.ceil(end / snapshot.durationSeconds * count) - 1);
  if (last < first) return snapshot;
  const bins = snapshot.bins.slice();
  for (let index = first; index <= last; index++) {
    const previous = bins[index];
    bins[index] = previous
      ? { min: Math.min(previous.min, segment.min), max: Math.max(previous.max, segment.max) }
      : { min: segment.min, max: segment.max };
  }
  return { ...snapshot, bins, reason: undefined };
}

/** Install complete local analysis only when it is explicitly supplied by the
 * caller for this exact resource. Partial local work remains incomplete.
 */
export function localWaveformSnapshot(input: {
  generation: number;
  trackId: string;
  durationSeconds: number;
  bins: Array<WaveformBin | null>;
  complete: boolean;
  scope?: "wholeTrack" | "playbackRange";
}): WaveformSnapshot {
  if (!Number.isSafeInteger(input.generation) || input.generation < 0 || !input.trackId || input.trackId.length > 512 ||
      !validDuration(input.durationSeconds) || !validCount(input.bins.length)) throw new Error("Invalid local waveform data.");
  for (const bin of input.bins) if (bin && !validAmplitude(bin.min, bin.max)) throw new Error("Invalid waveform amplitude.");
  const fullCoverage = input.bins.every(bin => bin !== null);
  if (input.complete && !fullCoverage) throw new Error("A complete waveform must cover the entire audio resource.");
  return { ...input, scope: input.scope ?? "wholeTrack", bins: input.bins.slice(), source: "local-analysis", complete: input.complete && fullCoverage };
}

/** Install a provider waveform only when the backend marks a complete
 * whole-resource analysis and supplies its stable, URL-free source identity.
 */
export function streamWaveformSnapshot(input: {
  generation: number;
  trackId: string;
  provider: string;
  sourceId: string;
  sourceIdentitySha256: string;
  durationSeconds: number;
  bins: Array<WaveformBin | null>;
  complete: boolean;
  scope: "wholeTrack";
}): WaveformSnapshot {
  if (!/^[A-Za-z0-9_-]{1,64}$/.test(input.provider) || !/^[A-Za-z0-9:_-]{1,512}$/.test(input.sourceId) ||
      !/^[a-f0-9]{64}$/i.test(input.sourceIdentitySha256) || input.scope !== "wholeTrack") {
    throw new Error("Invalid streamed waveform resource identity.");
  }
  if (!input.complete || input.bins.some(bin => bin === null)) throw new Error("Stream waveform is not complete.");
  const snapshot = localWaveformSnapshot(input);
  return { ...snapshot, source: "stream-analysis", complete: true, resource: {
    provider: input.provider, sourceId: input.sourceId, sourceIdentitySha256: input.sourceIdentitySha256,
  } };
}

/** Crop verified whole-track bins to a selected source-time interval and map
 * them onto the playback range's own seek axis. Missing cache bins remain
 * missing; source bins may be repeated when the selected range is shorter.
 */
export function cropWaveformBins(input: {
  bins: Array<WaveformBin | null>;
  durationSeconds: number;
  startSeconds: number;
  endSeconds: number;
  outputBinCount?: number;
}): Array<WaveformBin | null> {
  const outputCount = input.outputBinCount ?? input.bins.length;
  if (!validDuration(input.durationSeconds) || !validCount(input.bins.length) || !validCount(outputCount) ||
      !Number.isFinite(input.startSeconds) || !Number.isFinite(input.endSeconds) || input.startSeconds < 0 ||
      input.endSeconds <= input.startSeconds || input.endSeconds > input.durationSeconds + 1e-6) {
    throw new Error("Invalid waveform playback range.");
  }
  for (const bin of input.bins) if (bin && !validAmplitude(bin.min, bin.max)) throw new Error("Invalid waveform amplitude.");
  const start = Math.min(input.durationSeconds, input.startSeconds);
  const end = Math.min(input.durationSeconds, input.endSeconds);
  return Array.from({ length: outputCount }, (_, outputIndex) => {
    const targetStart = start + (end - start) * outputIndex / outputCount;
    const targetEnd = start + (end - start) * (outputIndex + 1) / outputCount;
    const first = Math.min(input.bins.length - 1, Math.floor(targetStart / input.durationSeconds * input.bins.length));
    const last = Math.min(input.bins.length - 1, Math.max(first, Math.ceil(targetEnd / input.durationSeconds * input.bins.length) - 1));
    let min = Infinity, max = -Infinity;
    for (let sourceIndex = first; sourceIndex <= last; sourceIndex++) {
      const bin = input.bins[sourceIndex];
      if (!bin) return null;
      min = Math.min(min, bin.min);
      max = Math.max(max, bin.max);
    }
    return Number.isFinite(min) && Number.isFinite(max) ? { min, max } : null;
  });
}

export function waveformCoverage(snapshot: WaveformSnapshot): { observedBins: number; totalBins: number; fraction: number } {
  const observedBins = snapshot.bins.reduce((count, bin) => count + (bin ? 1 : 0), 0);
  return { observedBins, totalBins: snapshot.bins.length, fraction: observedBins / snapshot.bins.length };
}

/** A compact explicit label; observed playback is never described as a full
 * track waveform, even after every visible bin happens to be populated.
 */
export function waveformStatus(snapshot: WaveformSnapshot): string {
  if (snapshot.source === "stream-analysis" && snapshot.complete) return "Full verified stream waveform";
  if (snapshot.source === "stream-analysis") return "Stream waveform analysis is incomplete; unmeasured regions remain blank.";
  if (snapshot.source === "local-analysis" && snapshot.scope === "playbackRange") return "Full waveform data for this playback range; the complete recording remains cached.";
  if (snapshot.source === "local-analysis" && snapshot.complete) return "Full audio waveform";
  if (snapshot.reason) return snapshot.reason;
  const coverage = waveformCoverage(snapshot);
  if (snapshot.source === "observed-playback" && snapshot.complete && coverage.observedBins === coverage.totalBins) {
    return "Complete playback observed from the start through verified natural end.";
  }
  if (!coverage.observedBins) return "No waveform audio observed yet; blank areas have not been analyzed.";
  return `Observed playback only · ${Math.round(coverage.fraction * 100)}% of the timeline sampled · blank areas have not been analyzed.`;
}
