import type { LiveBpmEstimate, LiveKeyEstimate } from "./types";

export const LIVE_MEASUREMENT_CACHE_NAMESPACE = "live-analysis-v1";
export const LIVE_MEASUREMENT_CACHE_SCHEMA = 1;

export type LiveMeasurementResource =
  | { kind: "stream"; provider: "youtube" | "soundcloud" | "bandcamp" | "monochrome"; sourceId: string; durationSeconds: number; recordingId?: string | null }
  | { kind: "localAsset"; assetId: string; fileHash: string; durationSeconds?: number; recordingId?: string | null };

export type PersistedLiveBpm = Pick<LiveBpmEstimate, "bpm" | "confidence" | "windowSeconds"> & {
  relatedTempoAlternativeBpm?: number | null;
};
export type PersistedLiveKey = Pick<LiveKeyEstimate, "key" | "profileFit" | "profileMargin" | "windowSeconds">;
export type TimestampedMeasurement<T> = {
  value: T;
  measuredAt: string;
  positionSeconds: number | null;
};
export type LiveMeasurementCacheRecord = {
  schemaVersion: typeof LIVE_MEASUREMENT_CACHE_SCHEMA;
  durationSeconds: number | null;
  bpm: TimestampedMeasurement<PersistedLiveBpm> | null;
  key: TimestampedMeasurement<PersistedLiveKey> | null;
};

export type LiveMeasurementCacheApi = {
  read: (namespace: string, key: string) => Promise<unknown>;
  write: (namespace: string, key: string, payload: LiveMeasurementCacheRecord, recordingId?: string | null) => Promise<void>;
};

const KEYS = new Set(["A", "Am", "Bb", "Bbm", "B", "Bm", "C", "Cm", "Db", "Dbm", "D", "Dm", "Eb", "Ebm", "E", "Em", "F", "Fm", "Gb", "Gbm", "G", "Gm", "Ab", "Abm"]);
const SAFE_ID = /^[A-Za-z0-9:_-]{1,512}$/;
const HASH = /^[a-f0-9]{64}$/i;

function canonicalResource(resource: LiveMeasurementResource): string | null {
  if (resource.kind === "stream") {
    if (!SAFE_ID.test(resource.sourceId) || resource.sourceId.includes("://") || /[?#]/.test(resource.sourceId) || !validDuration(resource.durationSeconds)) return null;
    return JSON.stringify(["stream", resource.provider, resource.sourceId]);
  }
  if (!SAFE_ID.test(resource.assetId) || !HASH.test(resource.fileHash) || (resource.durationSeconds !== undefined && !validDuration(resource.durationSeconds))) return null;
  return JSON.stringify(["localAsset", resource.assetId, resource.fileHash.toLowerCase()]);
}

function validDuration(value: unknown): value is number {
  return typeof value === "number" && Number.isFinite(value) && value > 0 && value <= 86_400;
}

function durationMatches(recordDuration: number | null, resource: LiveMeasurementResource): boolean {
  const current = resource.durationSeconds ?? null;
  if (current === null || recordDuration === null) return current === recordDuration;
  return Math.abs(current - recordDuration) <= Math.max(2, current * 0.01);
}

/** Hash stable provider/asset identity before it reaches the durable cache key. */
export async function liveMeasurementCacheKey(resource: LiveMeasurementResource): Promise<string | null> {
  const canonical = canonicalResource(resource);
  if (!canonical || !globalThis.crypto?.subtle) return null;
  const bytes = await crypto.subtle.digest("SHA-256", new TextEncoder().encode(`burrata-live-measurement-v1\n${canonical}`));
  return [...new Uint8Array(bytes)].map(byte => byte.toString(16).padStart(2, "0")).join("");
}

function validTimestamp(value: unknown): value is string {
  return typeof value === "string" && Number.isFinite(Date.parse(value)) && value.length <= 40;
}

function validPosition(value: unknown): value is number | null {
  return value === null || (typeof value === "number" && Number.isFinite(value) && value >= 0 && value <= 86_400);
}

function parseBpm(value: unknown): PersistedLiveBpm | null {
  if (!value || typeof value !== "object") return null;
  const item = value as Record<string, unknown>;
  if (typeof item.bpm !== "number" || !Number.isFinite(item.bpm) || item.bpm <= 0 || item.bpm > 400 ||
      typeof item.confidence !== "number" || !Number.isFinite(item.confidence) || item.confidence < 0 || item.confidence > 1 ||
      typeof item.windowSeconds !== "number" || !Number.isFinite(item.windowSeconds) || item.windowSeconds <= 0 || item.windowSeconds > 60 ||
      (item.relatedTempoAlternativeBpm != null && (typeof item.relatedTempoAlternativeBpm !== "number" || !Number.isFinite(item.relatedTempoAlternativeBpm) || item.relatedTempoAlternativeBpm <= 0 || item.relatedTempoAlternativeBpm > 400))) return null;
  return { bpm: item.bpm, confidence: item.confidence, windowSeconds: item.windowSeconds,
    ...(item.relatedTempoAlternativeBpm == null ? {} : { relatedTempoAlternativeBpm: item.relatedTempoAlternativeBpm }) };
}

function parseKey(value: unknown): PersistedLiveKey | null {
  if (!value || typeof value !== "object") return null;
  const item = value as Record<string, unknown>;
  if (typeof item.key !== "string" || !KEYS.has(item.key) ||
      typeof item.profileFit !== "number" || !Number.isFinite(item.profileFit) || item.profileFit < 0.2 || item.profileFit > 1 ||
      typeof item.profileMargin !== "number" || !Number.isFinite(item.profileMargin) || item.profileMargin < 0.01 || item.profileMargin > 2 ||
      typeof item.windowSeconds !== "number" || !Number.isFinite(item.windowSeconds) || item.windowSeconds < 8 || item.windowSeconds > 12.01) return null;
  return { key: item.key, profileFit: item.profileFit, profileMargin: item.profileMargin, windowSeconds: item.windowSeconds };
}

function parseSlot<T>(value: unknown, parseValue: (value: unknown) => T | null): TimestampedMeasurement<T> | null {
  if (!value || typeof value !== "object") return null;
  const item = value as Record<string, unknown>;
  const parsed = parseValue(item.value);
  return parsed && validTimestamp(item.measuredAt) && validPosition(item.positionSeconds)
    ? { value: parsed, measuredAt: new Date(item.measuredAt).toISOString(), positionSeconds: item.positionSeconds }
    : null;
}

export function validateLiveMeasurementCache(value: unknown): LiveMeasurementCacheRecord | null {
  if (!value || typeof value !== "object") return null;
  const item = value as Record<string, unknown>;
  if (item.schemaVersion !== LIVE_MEASUREMENT_CACHE_SCHEMA || (item.durationSeconds !== null && !validDuration(item.durationSeconds)) ||
      (item.bpm !== null && item.bpm === undefined) ||
      (item.key !== null && item.key === undefined)) return null;
  return {
    schemaVersion: LIVE_MEASUREMENT_CACHE_SCHEMA,
    durationSeconds: item.durationSeconds as number | null,
    bpm: item.bpm === null ? null : parseSlot(item.bpm, parseBpm),
    key: item.key === null ? null : parseSlot(item.key, parseKey),
  };
}

export async function readLiveMeasurementCache(
  resource: LiveMeasurementResource,
  api: LiveMeasurementCacheApi,
): Promise<LiveMeasurementCacheRecord | null> {
  const key = await liveMeasurementCacheKey(resource);
  if (!key) return null;
  try {
    const record = validateLiveMeasurementCache(await api.read(LIVE_MEASUREMENT_CACHE_NAMESPACE, key));
    return record && durationMatches(record.durationSeconds, resource) ? record : null;
  }
  catch { return null; }
}

/** Update only measurements present in this sample; absent BPM/key never erase a prior valid value. */
export async function saveLiveMeasurements(
  resource: LiveMeasurementResource,
  estimates: { bpm?: LiveBpmEstimate | null; key?: LiveKeyEstimate | null },
  measuredAt: string,
  positionSeconds: number | null,
  api: LiveMeasurementCacheApi,
): Promise<boolean> {
  if (!validTimestamp(measuredAt) || !validPosition(positionSeconds)) return false;
  const key = await liveMeasurementCacheKey(resource);
  if (!key) return false;
  const bpm = estimates.bpm?.trackId ? parseBpm(estimates.bpm) : null;
  const liveKey = estimates.key?.trackId ? parseKey(estimates.key) : null;
  if (!bpm && !liveKey) return false;
  const pending = saveQueues.get(key) ?? Promise.resolve(false);
  const result = pending.catch(() => false).then(async () => {
    const existing = await readLiveMeasurementCache(resource, api) ?? {
      schemaVersion: 1 as const,
      durationSeconds: resource.durationSeconds ?? null,
      bpm: null,
      key: null,
    };
    const next: LiveMeasurementCacheRecord = {
      schemaVersion: LIVE_MEASUREMENT_CACHE_SCHEMA,
      durationSeconds: resource.durationSeconds ?? null,
      bpm: bpm ? { value: bpm, measuredAt: new Date(measuredAt).toISOString(), positionSeconds } : existing.bpm,
      key: liveKey ? { value: liveKey, measuredAt: new Date(measuredAt).toISOString(), positionSeconds } : existing.key,
    };
    if (!next.bpm && !next.key) return false;
    await api.write(LIVE_MEASUREMENT_CACHE_NAMESPACE, key, next, resource.recordingId ?? null);
    return true;
  });
  saveQueues.set(key, result);
  try { return await result; }
  catch { return false; }
  finally { if (saveQueues.get(key) === result) saveQueues.delete(key); }
}

const saveQueues = new Map<string, Promise<boolean>>();
