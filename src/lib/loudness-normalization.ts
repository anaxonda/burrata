/** Shared Native normalization settings, resource identities and display labels. */

export type NormalizationMode = "off" | "track" | "album";
export type LoudnessStatus = "off" | "applied" | "album_fallback_track" | "unavailable";
export type LoudnessSettings = {
  mode: NormalizationMode;
  targetLufs: number;
  ceilingDbtp: number;
  maximumBoostDb: number;
  albumTrackFallback: boolean;
  analyzeWhenIdle: boolean;
};
export type LoudnessResourceIdentity = {
  scope: "track" | "album";
  members: Array<{
    assetId: string;
    fileHash: string;
    range: { startSeconds: number; endSeconds: number | null };
  }>;
  interTrackGapsSeconds: number[];
  resourceKey: string;
};
export type LoudnessAnalysisJobStatus = {
  jobId: string;
  status: "queued" | "running" | "completed" | "cancelled" | "failed" | string;
  total: number;
  completed: number;
  error: string | null;
  resource?: LoudnessResourceIdentity | null;
};
export type LoudnessPlaybackStatus = {
  mode?: NormalizationMode;
  appliedGainDb: number;
  status: LoudnessStatus;
  reason?: string | null;
  sourceResourceKey?: string | null;
  measurementSource?: string | null;
};

export function loudnessMeasurementSourceLabel(source?: string | null): string {
  const labels: Record<string, string> = {
    ffmpegAnalysis: "Audio analysis",
    replayGainTrack: "ReplayGain track tag",
    replayGainAlbum: "ReplayGain album tag",
    opusR128Track: "Opus track tag",
    opusR128Album: "Opus album tag",
  };
  if (source && Object.prototype.hasOwnProperty.call(labels, source)) return labels[source]!;
  if (source && Object.values(labels).includes(source)) return source;
  return "Audio measurement";
}

