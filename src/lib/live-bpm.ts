import type { LiveBpmEstimate } from "./types";

export type LiveBpmOwner = { generation: number; seekSerial: number; trackId: string };

/** Reject estimates from an earlier track, seek, or native playback generation. */
export function isCurrentLiveBpmEstimate(value: LiveBpmEstimate | null | undefined, owner: LiveBpmOwner): value is LiveBpmEstimate {
  return Boolean(value && Number.isSafeInteger(value.generation) && value.generation === owner.generation &&
    Number.isSafeInteger(value.seekSerial) && value.seekSerial === owner.seekSerial && value.trackId === owner.trackId &&
    Number.isFinite(value.bpm) && value.bpm > 0 && value.bpm <= 400 &&
    Number.isFinite(value.confidence) && value.confidence >= 0 && value.confidence <= 1 &&
    Number.isFinite(value.windowSeconds) && value.windowSeconds > 0);
}
