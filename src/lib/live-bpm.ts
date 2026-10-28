import type { LiveBpmEstimate, LiveKeyEstimate } from "./types";

export type LiveBpmOwner = { generation: number; seekSerial: number; trackId: string };

/** Reject estimates from an earlier track, seek, or native playback generation. */
export function isCurrentLiveBpmEstimate(value: LiveBpmEstimate | null | undefined, owner: LiveBpmOwner): value is LiveBpmEstimate {
  return Boolean(value && Number.isSafeInteger(value.generation) && value.generation === owner.generation &&
    Number.isSafeInteger(value.seekSerial) && value.seekSerial === owner.seekSerial && value.trackId === owner.trackId &&
    Number.isFinite(value.bpm) && value.bpm > 0 && value.bpm <= 400 &&
    Number.isFinite(value.confidence) && value.confidence >= 0 && value.confidence <= 1 &&
    Number.isFinite(value.windowSeconds) && value.windowSeconds > 0);
}

const LIVE_KEYS = new Set(["A", "Am", "Bb", "Bbm", "B", "Bm", "C", "Cm", "Db", "Dbm", "D", "Dm", "Eb", "Ebm", "E", "Em", "F", "Fm", "Gb", "Gbm", "G", "Gm", "Ab", "Abm"]);

/** Reject a provisional key from an earlier track, seek, or playback generation. */
export function isCurrentLiveKeyEstimate(value: LiveKeyEstimate | null | undefined, owner: LiveBpmOwner): value is LiveKeyEstimate {
  return Boolean(value && Number.isSafeInteger(value.generation) && value.generation === owner.generation &&
    Number.isSafeInteger(value.seekSerial) && value.seekSerial === owner.seekSerial && value.trackId === owner.trackId &&
    LIVE_KEYS.has(value.key) && Number.isFinite(value.profileFit) && value.profileFit >= .2 && value.profileFit <= 1 &&
    Number.isFinite(value.profileMargin) && value.profileMargin >= .01 && value.profileMargin <= 2 &&
    Number.isFinite(value.windowSeconds) && value.windowSeconds >= 8 && value.windowSeconds <= 12.01);
}

/** Only show an octave alternative supplied by the detector, never invent one. */
export function liveBpmLabel(value: Pick<LiveBpmEstimate, "bpm" | "relatedTempoAlternativeBpm">): string {
  const format = (bpm: number) => Number.isInteger(bpm) ? String(bpm) : bpm.toFixed(1);
  const alternative = value.relatedTempoAlternativeBpm;
  const tempos = typeof alternative === "number" && Number.isFinite(alternative) && alternative > 0 && alternative <= 400 && Math.abs(Math.max(value.bpm, alternative) / Math.min(value.bpm, alternative) - 2) < .08
    ? [value.bpm, alternative].sort((a,b)=>a-b) : [value.bpm];
  return `${tempos.map(format).join(" / ")} BPM`;
}
