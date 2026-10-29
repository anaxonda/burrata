import { describe, expect, it } from "vitest";
import {
  LIVE_MEASUREMENT_CACHE_NAMESPACE,
  liveMeasurementCacheKey,
  readLiveMeasurementCache,
  saveLiveMeasurements,
  validateLiveMeasurementCache,
  type LiveMeasurementCacheApi,
} from "./live-measurement-cache";
import type { LiveBpmEstimate, LiveKeyEstimate } from "./types";

const bpm: LiveBpmEstimate = { trackId: "track-1", generation: 4, seekSerial: 2, bpm: 123.5, confidence: 0.72, relatedTempoAlternativeBpm: 61.75, windowSeconds: 12 };
const key: LiveKeyEstimate = { trackId: "track-1", generation: 4, seekSerial: 2, key: "Am", profileFit: 0.43, profileMargin: 0.08, windowSeconds: 12 };
const resource = { kind: "stream" as const, provider: "youtube" as const, sourceId: "youtube:Abcdefghijk", durationSeconds: 180 };

function memoryApi(): LiveMeasurementCacheApi {
  const values = new Map<string, unknown>();
  const id = (namespace: string, key: string) => `${namespace}:${key}`;
  return {
    read: async (namespace, key) => values.get(id(namespace, key)) ?? null,
    write: async (namespace, key, payload) => { values.set(id(namespace, key), structuredClone(payload)); },
  };
}

describe("persistent live measurement cache", () => {
  it("uses opaque, distinct keys for exact provider source or registered asset hash", async () => {
    const streamKey = await liveMeasurementCacheKey(resource);
    expect(streamKey).toMatch(/^[a-f0-9]{64}$/);
    expect(await liveMeasurementCacheKey({ ...resource, sourceId: "youtube:DifferentID" })).not.toBe(streamKey);
    expect(await liveMeasurementCacheKey({ kind: "localAsset", assetId: "asset-1", fileHash: "a".repeat(64) })).toMatch(/^[a-f0-9]{64}$/);
    expect(await liveMeasurementCacheKey({ kind: "localAsset", assetId: "asset-1", fileHash: "b".repeat(64) })).not.toBe(await liveMeasurementCacheKey({ kind: "localAsset", assetId: "asset-1", fileHash: "a".repeat(64) }));
    expect(await liveMeasurementCacheKey({ ...resource, sourceId: "https://signed.invalid/audio" })).toBeNull();
  });

  it("stores BPM and key independently with their own last measured time and position", async () => {
    const api = memoryApi();
    expect(await saveLiveMeasurements(resource, { bpm }, "2026-10-28T10:00:00.000Z", 31.25, api)).toBe(true);
    expect(await saveLiveMeasurements(resource, { key }, "2026-10-28T10:00:05.000Z", 36.25, api)).toBe(true);
    const loaded = await readLiveMeasurementCache(resource, api);
    expect(loaded).toEqual({
      schemaVersion: 1,
      durationSeconds: 180,
      bpm: { value: { bpm: 123.5, confidence: 0.72, relatedTempoAlternativeBpm: 61.75, windowSeconds: 12 }, measuredAt: "2026-10-28T10:00:00.000Z", positionSeconds: 31.25 },
      key: { value: { key: "Am", profileFit: 0.43, profileMargin: 0.08, windowSeconds: 12 }, measuredAt: "2026-10-28T10:00:05.000Z", positionSeconds: 36.25 },
    });
    const keyHash = await liveMeasurementCacheKey(resource);
    expect(JSON.stringify(await api.read(LIVE_MEASUREMENT_CACHE_NAMESPACE, keyHash!))).not.toContain("generation");
    expect(JSON.stringify(await api.read(LIVE_MEASUREMENT_CACHE_NAMESPACE, keyHash!))).not.toContain("sourceId");
  });

  it("does not replace a valid metric with an absent or invalid estimate", async () => {
    const api = memoryApi();
    await saveLiveMeasurements(resource, { bpm }, "2026-10-28T10:00:00Z", 2, api);
    expect(await saveLiveMeasurements(resource, { key: { ...key, key: "H" } }, "2026-10-28T10:01:00Z", 3, api)).toBe(false);
    const loaded = await readLiveMeasurementCache(resource, api);
    expect(loaded?.bpm?.value.bpm).toBe(123.5);
    expect(loaded?.key).toBeNull();
  });

  it("rejects malformed cache slots and out-of-range timestamps or positions", async () => {
    expect(validateLiveMeasurementCache({ schemaVersion: 1, durationSeconds: 180, bpm: { value: { bpm: 0 }, measuredAt: "not a date", positionSeconds: -1 }, key: null }))
      .toEqual({ schemaVersion: 1, durationSeconds: 180, bpm: null, key: null });
    const api = memoryApi();
    expect(await saveLiveMeasurements(resource, { bpm }, "not a date", 1, api)).toBe(false);
    expect(await saveLiveMeasurements(resource, { bpm }, "2026-10-28T10:00:00Z", Number.NaN, api)).toBe(false);
    expect(await readLiveMeasurementCache(resource, api)).toBeNull();
  });

  it("keeps valid local asset measurements scoped to the verified asset hash", async () => {
    const api = memoryApi();
    const local = { kind: "localAsset" as const, assetId: "asset-1", fileHash: "c".repeat(64) };
    await saveLiveMeasurements(local, { bpm }, "2026-10-28T10:00:00Z", null, api);
    expect((await readLiveMeasurementCache(local, api))?.bpm?.positionSeconds).toBeNull();
    expect(await readLiveMeasurementCache({ ...local, fileHash: "d".repeat(64) }, api)).toBeNull();
  });

  it("invalidates a stream cache when its source duration changes beyond tolerance", async () => {
    const api = memoryApi();
    await saveLiveMeasurements(resource, { bpm }, "2026-10-28T10:00:00Z", 2, api);
    expect(await readLiveMeasurementCache({ ...resource, durationSeconds: 184 }, api)).toBeNull();
    expect((await readLiveMeasurementCache({ ...resource, durationSeconds: 181.5 }, api))?.bpm?.value.bpm).toBe(123.5);
  });
});
