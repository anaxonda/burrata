import { afterEach, describe, expect, it, vi } from "vitest";
import { ObservedWaveformCache, OBSERVED_WAVEFORM_CACHE_MAX_ENTRIES, OBSERVED_WAVEFORM_CACHE_MAX_SESSION_BYTES, OBSERVED_WAVEFORM_CACHE_STORAGE_KEY, OBSERVED_WAVEFORM_CACHE_WRITE_DELAY_MS, prepareObservedWaveformBinding, type ObservedWaveformCacheBinding } from "./observed-waveform-cache";
import { emptyWaveform, localWaveformSnapshot, type WaveformBin, type WaveformSnapshot } from "./waveform-data";

class MemoryStorage {
  value: string | null = null;
  failWrites = 0;
  setCalls = 0;
  removeCalls = 0;
  getItem(key: string): string | null { return key === OBSERVED_WAVEFORM_CACHE_STORAGE_KEY ? this.value : null; }
  setItem(key: string, value: string): void {
    if (key !== OBSERVED_WAVEFORM_CACHE_STORAGE_KEY) return;
    this.setCalls++;
    if (this.failWrites > 0) { this.failWrites--; throw new Error("quota"); }
    this.value = value;
  }
  removeItem(key: string): void { if (key === OBSERVED_WAVEFORM_CACHE_STORAGE_KEY) { this.removeCalls++; this.value = null; } }
}

function binding(overrides: Partial<ObservedWaveformCacheBinding> = {}): ObservedWaveformCacheBinding {
  return {
    resourceKey: "opaque-resource-track-a-youtube-video-a-30s",
    trackId: "track-a",
    generation: 1,
    durationSeconds: 30,
    binCount: 4,
    scope: "wholeTrack",
    range: null,
    identity: {
      kind: "stream", provider: "youtube", sourceId: "video-a",
      audioVariant: { formatId: "140", codec: "mp4a.40.2", mimeType: "audio/mp4", expectedContentLength: 2048, containsVideo: false },
    },
    ...overrides,
  };
}

function partial(prepared: Awaited<ReturnType<typeof prepareObservedWaveformBinding>>, values: Array<WaveformBin | null>): WaveformSnapshot {
  if (!prepared) throw new Error("test binding did not prepare");
  const snapshot = emptyWaveform(prepared.generation, prepared.trackId, prepared.durationSeconds, prepared.binCount);
  snapshot.bins = values;
  snapshot.scope = prepared.scope;
  return snapshot;
}

describe("ObservedWaveformCache", () => {
  it("accepts the helper's full 24-hour duration bound", async () => {
    const prepared = await prepareObservedWaveformBinding(binding({ durationSeconds: 86_400, identity: { kind: "session" } }));
    expect(prepared?.durationSeconds).toBe(86_400);
  });
  afterEach(() => vi.useRealTimers());

  it("restores partial bins across generations only for the exact stable resource", async () => {
    const storage = new MemoryStorage();
    const first = await prepareObservedWaveformBinding(binding());
    expect(first?.persistenceKey).toMatch(/^[a-f0-9]{64}$/);
    const writer = new ObservedWaveformCache(storage);
    const written = writer.record(first!, partial(first, [{ min: -0.4, max: 0.5 }, null, { min: -0.2, max: 0.3 }, null]));
    expect(written?.complete).toBe(false);
    expect(writer.flush()).toBe(true);
    expect(storage.value).not.toContain("video-a");
    expect(storage.value).not.toContain("track-a");

    const second = await prepareObservedWaveformBinding(binding({ generation: 2 }));
    const restarted = new ObservedWaveformCache(storage);
    const restored = restarted.restore(second!);
    expect(restored).toMatchObject({ generation: 2, trackId: "track-a", durationSeconds: 30, source: "observed-playback", complete: false });
    expect(restored?.bins).toEqual([{ min: -0.4, max: 0.5 }, null, { min: -0.2, max: 0.3 }, null]);

    const wrongSource = await prepareObservedWaveformBinding(binding({ generation: 2, resourceKey: "opaque-resource-track-a-youtube-video-b-30s", identity: { kind: "stream", provider: "youtube", sourceId: "video-b", audioVariant: { formatId: "140", codec: "mp4a.40.2" } } }));
    expect(restarted.restore(wrongSource!)).toBeNull();
  });

  it("imports and exports a database record only for its exact durable key", async () => {
    const binding1 = await prepareObservedWaveformBinding(binding());
    const writer = new ObservedWaveformCache(new MemoryStorage());
    writer.record(binding1!, partial(binding1, [{ min: -0.2, max: 0.2 }, null, null, null]));
    const row = writer.exportPersistedRecord(binding1!);
    const binding2 = await prepareObservedWaveformBinding(binding({ generation: 2 }));
    const reader = new ObservedWaveformCache(new MemoryStorage());
    expect(reader.restorePersistedRecord(binding2!, row)?.bins[0]).toEqual({ min: -0.2, max: 0.2 });
    const wrong = await prepareObservedWaveformBinding(binding({
      resourceKey: "different-resource",
      identity: { kind: "stream", provider: "youtube", sourceId: "different-recording", audioVariant: { formatId: "140", codec: "mp4a.40.2" } },
    }));
    expect(reader.restorePersistedRecord(wrong!, row)).toBeNull();
    const corrupt = { ...(row as Record<string, unknown>), bins: ["bad"] };
    expect(reader.restorePersistedRecord(binding2!, corrupt)).toBeNull();
  });

  it("defers full-cache serialization until flush while samples change", async () => {
    const storage = new MemoryStorage();
    const prepared = await prepareObservedWaveformBinding(binding());
    const cache = new ObservedWaveformCache(storage);
    const serialize = vi.spyOn(JSON, "stringify");
    try {
      for (let i = 1; i <= 10; i++) cache.record(prepared!, partial(prepared, [{ min: -i / 10, max: i / 10 }, null, null, null]));
      expect(serialize).not.toHaveBeenCalled();
      expect(storage.setCalls).toBe(0);
      expect(cache.flush()).toBe(true);
      expect(serialize).toHaveBeenCalled();
      expect(storage.setCalls).toBe(1);
    } finally { serialize.mockRestore(); }
  });

  it("merges a delayed database restore with observations already recorded in the session", async () => {
    const storage = new MemoryStorage();
    const prepared = (await prepareObservedWaveformBinding(binding()))!;
    const old = new ObservedWaveformCache(new MemoryStorage());
    old.record(prepared, partial(prepared,[{min:-0.4,max:0.5},null,null,null]));
    const persisted = old.exportPersistedRecord(prepared);
    const cache = new ObservedWaveformCache(storage);
    cache.record(prepared,partial(prepared,[null,{min:-0.2,max:0.3},null,null]),{flushNow:true});
    cache.restorePersistedRecord(prepared,persisted);
    cache.flush();
    expect(cache.restore(prepared)?.bins.slice(0,2)).toEqual([{min:-0.4,max:0.5},{min:-0.2,max:0.3}]);
    const stored=JSON.parse(storage.value!);
    expect(stored.records[0].bins.slice(0,2)).toEqual([[-0.4,0.5],[-0.2,0.3]]);
  });

  it("does not rewrite unchanged paused coverage and retries a failed flush", async () => {
    const storage = new MemoryStorage();
    const prepared = await prepareObservedWaveformBinding(binding());
    const cache = new ObservedWaveformCache(storage);
    const snapshot = partial(prepared, [{ min: -0.2, max: 0.3 }, null, null, null]);
    storage.failWrites = 1;
    cache.record(prepared!, snapshot, { flushNow: true });
    expect(storage.setCalls).toBe(1);
    const version = cache.persistedVersion(prepared!);
    expect(version).not.toBeNull();
    cache.record(prepared!, snapshot, { flushNow: true });
    expect(cache.persistedVersion(prepared!)).toBe(version);
    expect(storage.setCalls).toBe(2);
    for (let i = 0; i < 10; i++) cache.record(prepared!, snapshot, { flushNow: true });
    expect(storage.setCalls).toBe(2);
    cache.record(prepared!, partial(prepared, [null, { min: -0.1, max: 0.1 }, null, null]), { flushNow: true });
    expect(storage.setCalls).toBe(3);
  });

  it("persists stable recording identities across encoding changes, including Monochrome", async () => {
    const input = binding({ identity: { kind: "stream", provider: "monochrome", sourceId: "catalog-a",
      audioVariant: { formatId: "lossless", codec: "flac", mimeType: "audio/flac", expectedContentLength: 2048 } } });
    const prepared = await prepareObservedWaveformBinding(input);
    expect(prepared?.persistenceKey).toMatch(/^[a-f0-9]{64}$/);
    const storage = new MemoryStorage();
    const cache = new ObservedWaveformCache(storage);
    cache.record(prepared!, partial(prepared, [{ min: -0.1, max: 0.2 }, null, null, null]), { flushNow: true });
    expect(cache.restore(prepared!)).not.toBeNull();
    const changedEncoding = await prepareObservedWaveformBinding({ ...input, identity: { kind: "stream", provider: "monochrome", sourceId: "catalog-a",
      audioVariant: { formatId: "lossy", codec: "aac", mimeType: "audio/mp4", expectedContentLength: 2048 } } });
    expect(cache.restore(changedEncoding!)?.bins[0]).toEqual({ min: -0.1, max: 0.2 });
    expect(cache.restore((await prepareObservedWaveformBinding({ ...input, generation: 2 }))!)?.bins[0]).toEqual({ min: -0.1, max: 0.2 });
    expect(storage.setCalls).toBeGreaterThan(0);
  });

  it("survives encoding changes but rejects duration, range, and resource changes", async () => {
    const storage = new MemoryStorage();
    const original = binding();
    const prepared = await prepareObservedWaveformBinding(original);
    const cache = new ObservedWaveformCache(storage);
    cache.record(prepared!, partial(prepared, [{ min: -0.2, max: 0.2 }, null, null, null]));
    cache.flush();

    const changedVariant = await prepareObservedWaveformBinding(binding({
      resourceKey: original.resourceKey,
      identity: { kind: "stream", provider: "youtube", sourceId: "video-a", audioVariant: { formatId: "251", codec: "opus", mimeType: "audio/webm", expectedContentLength: 2048, containsVideo: false } },
    }));
    const changedDuration = await prepareObservedWaveformBinding(binding({ durationSeconds: 31, resourceKey: "opaque-resource-track-a-youtube-video-a-31s" }));
    const changedRange = await prepareObservedWaveformBinding(binding({
      durationSeconds: 28, scope: "playbackRange", range: { startSeconds: 2, endSeconds: 30 }, resourceKey: "opaque-resource-track-a-youtube-video-a-2-30",
    }));
    const restarted = new ObservedWaveformCache(storage);
    expect(restarted.restore(changedVariant!)?.bins[0]).toEqual({ min: -0.2, max: 0.2 });
    expect(restarted.restore(changedDuration!)).toBeNull();
    expect(restarted.restore(changedRange!)).toBeNull();
  });

  it("reuses stream partials across track aliases and temporary resource-key changes", async () => {
    const storage = new MemoryStorage();
    const firstBinding = binding();
    const first = await prepareObservedWaveformBinding(firstBinding);
    const writer = new ObservedWaveformCache(storage);
    writer.record(first!, partial(first, [{ min: -0.25, max: 0.35 }, null, null, null]));
    writer.flush();
    const alias = await prepareObservedWaveformBinding(binding({
      resourceKey: "refreshed-session-resource-key",
      trackId: "track-alias",
      generation: 4,
      identity: { kind: "stream", provider: "youtube", sourceId: "video-a", audioVariant: { formatId: "251", codec: "opus" } },
    }));
    expect(alias?.persistenceKey).toBe(first?.persistenceKey);
    expect(new ObservedWaveformCache(storage).restore(alias!)?.bins[0]).toEqual({ min: -0.25, max: 0.35 });
  });

  it("reuses stream partials across duration padding but rejects material duration changes", async () => {
    const storage = new MemoryStorage();
    const first = await prepareObservedWaveformBinding(binding({
      identity: { kind: "stream", provider: "youtube", sourceId: "video-a", audioVariant: null },
    }));
    const writer = new ObservedWaveformCache(storage);
    writer.record(first!, partial(first, [{ min: -0.25, max: 0.35 }, null, null, null]));
    writer.flush();

    const padded = await prepareObservedWaveformBinding(binding({
      durationSeconds: 30.000117,
      resourceKey: "same-recording-webview-duration-padding",
      generation: 9,
      identity: { kind: "stream", provider: "youtube", sourceId: "video-a", audioVariant: null },
    }));
    expect(padded?.persistenceKey).toBe(first?.persistenceKey);
    expect(new ObservedWaveformCache(storage).restore(padded!)?.bins[0]).toEqual({ min: -0.25, max: 0.35 });
    const materiallyDifferent = await prepareObservedWaveformBinding(binding({
      durationSeconds: 31,
      resourceKey: "same-recording-material-duration-change",
      identity: { kind: "stream", provider: "youtube", sourceId: "video-a", audioVariant: null },
    }));
    expect(materiallyDifferent?.persistenceKey).toBe(first?.persistenceKey);
    expect(new ObservedWaveformCache(storage).restore(materiallyDifferent!)).toBeNull();
  });

  it("migrates exact-duration v1 stream keys and accepts padded database records", async () => {
    const storage = new MemoryStorage();
    const now = Date.now();
    const oldBinding = await prepareObservedWaveformBinding(binding({
      identity: { kind: "stream", provider: "youtube", sourceId: "video-a", audioVariant: null },
    }));
    const oldKey = oldBinding!.legacyPersistenceKeys![0];
    storage.value = JSON.stringify({
      schemaVersion: 1,
      records: [{
        key: oldKey, updatedAt: now, expiresAt: now + 7 * 24 * 60 * 60 * 1000,
        durationSeconds: 30, binCount: 4, scope: "wholeTrack", range: null,
        bins: [[-0.2, 0.2], null, null, null],
      }],
    });
    const migrated = new ObservedWaveformCache(storage, () => now);
    expect(migrated.restore(oldBinding!)?.bins[0]).toEqual({ min: -0.2, max: 0.2 });
    expect(migrated.exportPersistedRecord(oldBinding!)).toMatchObject({ key: oldBinding!.persistenceKey });

    const padded = await prepareObservedWaveformBinding(binding({
      durationSeconds: 30.000117,
      identity: { kind: "stream", provider: "youtube", sourceId: "video-a", audioVariant: null },
    }));
    const row = migrated.exportPersistedRecord(oldBinding!);
    expect(migrated.restorePersistedRecord(padded!, row)?.bins[0]).toEqual({ min: -0.2, max: 0.2 });
    const incompatible = await prepareObservedWaveformBinding(binding({
      durationSeconds: 31,
      identity: { kind: "stream", provider: "youtube", sourceId: "video-a", audioVariant: null },
    }));
    expect(migrated.restorePersistedRecord(incompatible!, row)).toBeNull();
  });

  it("retains clip-relative bins bound to absolute parent-source ranges", async () => {
    const storage = new MemoryStorage();
    const clipped = binding({ durationSeconds: 10, scope: "playbackRange", range: { startSeconds: 100, endSeconds: 110 } });
    const prepared = await prepareObservedWaveformBinding(clipped);
    expect(prepared).not.toBeNull();
    const cache = new ObservedWaveformCache(storage);
    cache.record(prepared!, partial(prepared, [null, { min: -0.2, max: 0.4 }, null, null]), { flushNow: true });
    const next = await prepareObservedWaveformBinding({ ...clipped, generation: 2 });
    expect(new ObservedWaveformCache(storage).restore(next!)?.bins[1]).toEqual({ min: -0.2, max: 0.4 });
    const otherRange = await prepareObservedWaveformBinding({ ...clipped, range: { startSeconds: 200, endSeconds: 210 } });
    expect(new ObservedWaveformCache(storage).restore(otherRange!)).toBeNull();
    expect(await prepareObservedWaveformBinding({ ...clipped, durationSeconds: 20 })).toBeNull();
  });

  it("merges only known bins and leaves every unobserved gap null", async () => {
    const prepared = await prepareObservedWaveformBinding(binding({ identity: { kind: "stream", provider: "youtube", sourceId: "video-a", audioVariant: null } }));
    expect(prepared?.persistenceKey).toMatch(/^[a-f0-9]{64}$/);
    const cache = new ObservedWaveformCache(null);
    cache.record(prepared!, partial(prepared, [{ min: -0.4, max: 0.4 }, null, null, { min: -0.1, max: 0.1 }]));
    const result = cache.record(prepared!, partial(prepared, [null, { min: -0.3, max: 0.3 }, null, { min: -0.2, max: 0.2 }]));
    expect(result?.bins).toEqual([
      { min: -0.4, max: 0.4 }, { min: -0.3, max: 0.3 }, null, { min: -0.2, max: 0.2 },
    ]);
    expect(result?.complete).toBe(false);
    expect(cache.restore(prepared!)?.bins).toEqual(result?.bins);
    const newGeneration = await prepareObservedWaveformBinding(binding({ generation: 2, identity: { kind: "stream", provider: "youtube", sourceId: "video-a", audioVariant: null } }));
    expect(cache.restore(newGeneration!)).toBeNull();
  });

  it("uses a verified local file hash for durable identity and rejects invalid hashes", async () => {
    const local = binding({
      resourceKey: "local-asset-a-file-hash-bound",
      identity: { kind: "local", assetId: "asset-a", verifiedFileHash: "a".repeat(64) },
    });
    const prepared = await prepareObservedWaveformBinding(local);
    expect(prepared?.persistenceKey).toMatch(/^[a-f0-9]{64}$/);
    expect(await prepareObservedWaveformBinding({ ...local, identity: { kind: "local", assetId: "asset-a", verifiedFileHash: "unverified" } })).toBeNull();
  });

  it("reuses local partial bins across asset-ID remapping only with the opaque verified resource key", async () => {
    const storage = new MemoryStorage();
    const contentKey = "c".repeat(64);
    const first = await prepareObservedWaveformBinding(binding({
      resourceKey: contentKey,
      identity: { kind: "localResource", resourceKeySha256: contentKey },
    }));
    expect(first?.persistenceKey).toMatch(/^[a-f0-9]{64}$/);
    const writer = new ObservedWaveformCache(storage);
    writer.record(first!, partial(first, [{ min: -0.3, max: 0.4 }, null, null, null]));
    writer.flush();

    const remapped = await prepareObservedWaveformBinding(binding({
      resourceKey: contentKey,
      trackId: "new-library-listing-id",
      generation: 9,
      identity: { kind: "localResource", resourceKeySha256: contentKey },
    }));
    expect(remapped?.persistenceKey).toBe(first?.persistenceKey);
    expect(new ObservedWaveformCache(storage).restore(remapped!)).toMatchObject({
      trackId: "new-library-listing-id", generation: 9, complete: false, source: "observed-playback",
    });
    expect(await prepareObservedWaveformBinding(binding({
      resourceKey: "d".repeat(64),
      identity: { kind: "localResource", resourceKeySha256: "not-a-server-digest" },
    }))).toBeNull();
  });

  it("retains valid data across long pauses and rejects a bin count outside the bounded model", async () => {
    let now = 1_800_000_000_000;
    const storage = new MemoryStorage();
    const prepared = await prepareObservedWaveformBinding(binding());
    const writer = new ObservedWaveformCache(storage, () => now);
    writer.record(prepared!, partial(prepared, [{ min: -0.1, max: 0.1 }, null, null, null]));
    writer.flush();
    now += 7 * 24 * 60 * 60 * 1000 + 1;
    expect(new ObservedWaveformCache(storage, () => now).restore(prepared!)?.bins[0]).toEqual({ min: -0.1, max: 0.1 });
    expect(await prepareObservedWaveformBinding(binding({ binCount: 4097 }))).toBeNull();
  });

  it("persists recording identity when stream variant metadata is unavailable", async () => {
    const storage = new MemoryStorage();
    const unknown = await prepareObservedWaveformBinding(binding({ identity: { kind: "stream", provider: "youtube", sourceId: "video-a", audioVariant: null } }));
    expect(unknown?.persistenceKey).toMatch(/^[a-f0-9]{64}$/);
    const cache = new ObservedWaveformCache(storage);
    cache.record(unknown!, partial(unknown, [{ min: -0.1, max: 0.1 }, null, null, null]));
    expect(cache.restore(unknown!)?.bins[0]).toEqual({ min: -0.1, max: 0.1 });
    expect(cache.flush()).toBe(true);
    expect(storage.setCalls).toBeGreaterThan(0);
    const nextGeneration = await prepareObservedWaveformBinding(binding({ generation: 2, identity: { kind: "stream", provider: "youtube", sourceId: "video-a", audioVariant: null } }));
    expect(new ObservedWaveformCache(storage).restore(nextGeneration!)?.bins[0]).toEqual({ min: -0.1, max: 0.1 });
  });

  it("never records a complete analyzed waveform over partial observed state", async () => {
    const prepared = await prepareObservedWaveformBinding(binding());
    const cache = new ObservedWaveformCache(null);
    cache.record(prepared!, partial(prepared, [{ min: -0.2, max: 0.2 }, null, null, null]));
    const full = localWaveformSnapshot({
      generation: prepared!.generation, trackId: prepared!.trackId, durationSeconds: prepared!.durationSeconds,
      bins: Array.from({ length: prepared!.binCount }, () => ({ min: -0.5, max: 0.5 })), complete: true,
    });
    expect(cache.record(prepared!, full)).toBeNull();
    expect(cache.restore(prepared!)?.bins).toEqual([{ min: -0.2, max: 0.2 }, null, null, null]);
  });

  it("coalesces persistent writes and retries a quota failure without discarding dirty bins", async () => {
    vi.useFakeTimers();
    const storage = new MemoryStorage();
    const prepared = await prepareObservedWaveformBinding(binding());
    const cache = new ObservedWaveformCache(storage, () => Date.now());
    cache.record(prepared!, partial(prepared, [{ min: -0.2, max: 0.2 }, null, null, null]));
    vi.advanceTimersByTime(OBSERVED_WAVEFORM_CACHE_WRITE_DELAY_MS - 1);
    expect(storage.setCalls).toBe(0);
    vi.advanceTimersByTime(1);
    expect(storage.setCalls).toBe(1);

    storage.failWrites = 1;
    cache.record(prepared!, partial(prepared, [null, { min: -0.3, max: 0.3 }, null, null]));
    vi.advanceTimersByTime(OBSERVED_WAVEFORM_CACHE_WRITE_DELAY_MS);
    expect(storage.setCalls).toBe(2);
    expect(cache.restore(prepared!)?.bins.slice(0, 2)).toEqual([{ min: -0.2, max: 0.2 }, { min: -0.3, max: 0.3 }]);
    vi.advanceTimersByTime(OBSERVED_WAVEFORM_CACHE_WRITE_DELAY_MS);
    expect(storage.setCalls).toBe(3);
    expect(new ObservedWaveformCache(storage).restore(prepared!)?.bins.slice(0, 2)).toEqual([{ min: -0.2, max: 0.2 }, { min: -0.3, max: 0.3 }]);
  });

  it("evicts oldest entries at the configured bound and recovers from corrupt storage", async () => {
    const storage = new MemoryStorage();
    const cache = new ObservedWaveformCache(storage);
    const bindings = await Promise.all(Array.from({ length: OBSERVED_WAVEFORM_CACHE_MAX_ENTRIES + 1 }, (_, index) => {
      const sourceId = `video-${index}`;
      return prepareObservedWaveformBinding(binding({ resourceKey: `opaque-${sourceId}`, identity: {
        kind: "stream", provider: "youtube", sourceId, audioVariant: { formatId: "140", codec: "mp4a.40.2" },
      } }));
    }));
    for (const item of bindings) cache.record(item!, partial(item, [{ min: -0.1, max: 0.1 }, null, null, null]));
    expect(cache.persistedEntryCount).toBeLessThanOrEqual(OBSERVED_WAVEFORM_CACHE_MAX_ENTRIES);
    cache.flush();
    const reloaded = new ObservedWaveformCache(storage);
    expect(reloaded.restore(bindings[0]!)).toBeNull();
    expect(reloaded.restore(bindings[bindings.length - 1]!)).not.toBeNull();

    storage.value = "not-json";
    const corrupt = new ObservedWaveformCache(storage);
    expect(corrupt.restore(bindings[bindings.length - 1]!)).toBeNull();
    expect(storage.removeCalls).toBeGreaterThan(0);
  });

  it("enforces the serialized byte ceiling even before the entry-count ceiling", async () => {
    const storage = new MemoryStorage();
    const cache = new ObservedWaveformCache(storage);
    const prepared = await Promise.all(Array.from({ length: 10 }, (_, index) => prepareObservedWaveformBinding(binding({
      resourceKey: `large-opaque-resource-${index}`,
      binCount: 4096,
      identity: { kind: "stream", provider: "youtube", sourceId: `large-${index}`, audioVariant: { formatId: "140", codec: "mp4a.40.2" } },
    }))));
    const bins = Array.from({ length: 4096 }, () => ({ min: -0.123456789, max: 0.987654321 }));
    for (const item of prepared) cache.record(item!, partial(item, bins));
    cache.flush();
    const bytes = new TextEncoder().encode(storage.value ?? "").byteLength;
    expect(bytes).toBeLessThanOrEqual(1_048_576);
    expect(cache.persistedEntryCount).toBeLessThan(OBSERVED_WAVEFORM_CACHE_MAX_ENTRIES);
  });

  it("bounds memory-only sessions by estimated bytes as well as entry count", async () => {
    const cache = new ObservedWaveformCache(null);
    const bins = Array.from({ length: 4096 }, () => ({ min: -0.1, max: 0.1 }));
    const prepared = await Promise.all(Array.from({ length: 12 }, (_, index) => prepareObservedWaveformBinding(binding({
      resourceKey: `session-only-large-${index}`,
      binCount: 4096,
      identity: { kind: "stream", provider: "youtube", sourceId: `session-${index}`, audioVariant: null },
    }))));
    for (const item of prepared) cache.record(item!, partial(item, bins));
    expect(cache.memoryApproxBytes).toBeLessThanOrEqual(OBSERVED_WAVEFORM_CACHE_MAX_SESSION_BYTES);
    expect(cache.memoryEntryCount).toBeLessThan(12);
    expect(cache.restore(prepared[prepared.length - 1]!)).not.toBeNull();
    expect(cache.restore(prepared[0]!)).toBeNull();
  });
});
