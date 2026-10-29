export interface PlayingTrackDetectionOwner {
  trackId: string;
  generation: number;
  seekSerial: number;
}

export interface DetectedTrackObservation {
  title: string;
  artist: string;
  detectedAtSeconds: number;
  /** Latest confirmation timestamp; used to estimate the observed song span. */
  lastConfirmedAtSeconds?: number;
  capturedStartSeconds: number;
  capturedEndSeconds: number;
  observedAt: string;
  artworkUrl?: string | null;
  identifiers?: { isrc?: string | null; shazamId?: string | null } | null;
}

export function activeDetectedTrack(observations: DetectedTrackObservation[], positionSeconds: number): DetectedTrackObservation | null {
  if (!Number.isFinite(positionSeconds) || positionSeconds < 0) return null;
  let active: DetectedTrackObservation | null = null;
  for (const observation of observations) {
    if (observation.detectedAtSeconds <= positionSeconds && (!active || observation.detectedAtSeconds > active.detectedAtSeconds)) active = observation;
  }
  return active;
}

export interface PlayingTrackDetectionView {
  /** Opaque server-derived identity for the exact active audio resource. */
  resourceKey: string;
  enabled: boolean;
  observations: DetectedTrackObservation[];
  retentionLimitReached?: boolean;
}

export const PLAYING_TRACK_DETECTION_INTERVAL_MS = 35_000;

export function samePlayingTrackDetectionOwner(
  expected: PlayingTrackDetectionOwner | null,
  current: PlayingTrackDetectionOwner | null,
): boolean {
  return Boolean(
    expected && current &&
      expected.trackId === current.trackId &&
      expected.generation === current.generation &&
      expected.seekSerial === current.seekSerial,
  );
}

export function playingTrackDetectionOwnerKey(owner: PlayingTrackDetectionOwner | null): string | null {
  return owner ? JSON.stringify([owner.trackId, owner.generation, owner.seekSerial]) : null;
}

export function approximateDetectionTime(seconds: number): string {
  if (!Number.isFinite(seconds) || seconds < 0) return "time unavailable";
  const whole = Math.floor(seconds);
  const hours = Math.floor(whole / 3600);
  const minutes = Math.floor((whole % 3600) / 60);
  const remainder = whole % 60;
  const clock = hours > 0
    ? `${hours}:${String(minutes).padStart(2, "0")}:${String(remainder).padStart(2, "0")}`
    : `${minutes}:${String(remainder).padStart(2, "0")}`;
  return `~${clock}`;
}
