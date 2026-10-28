import { streamAudioVariantIdentity, type StreamAudioVariant } from "./stream-media-analysis";
import { MAX_WAVEFORM_BIN_COUNT, emptyWaveform, type WaveformBin, type WaveformSnapshot } from "./waveform-data";

export const OBSERVED_WAVEFORM_CACHE_STORAGE_KEY = "burrata.observed-waveform-cache.v1";
export const OBSERVED_WAVEFORM_CACHE_TTL_MS = 7 * 24 * 60 * 60 * 1000;
export const OBSERVED_WAVEFORM_CACHE_MAX_ENTRIES = 16;
export const OBSERVED_WAVEFORM_CACHE_MAX_SESSION_BYTES = 2 * 1024 * 1024;
export const OBSERVED_WAVEFORM_CACHE_MAX_SERIALIZED_BYTES = 1024 * 1024;
export const OBSERVED_WAVEFORM_CACHE_WRITE_DELAY_MS = 15_000;

export type ObservedWaveformCacheBinding = {
  /** Current exact-resource key; it is hashed before any persistent write. */
  resourceKey: string;
  trackId: string;
  generation: number;
  durationSeconds: number;
  binCount: number;
  scope: "wholeTrack" | "playbackRange";
  /** Absolute parent-source bounds; durationSeconds is the displayed clip length. */
  range: { startSeconds: number; endSeconds: number } | null;
  identity:
    | { kind: "stream"; provider: string; sourceId: string; audioVariant: StreamAudioVariant | null }
    | { kind: "local"; assetId: string; verifiedFileHash: string }
    | { kind: "localResource"; resourceKeySha256: string }
    | { kind: "session" };
};

/** A binding is prepared once for a playback owner. Its persistenceKey is an
 * opaque digest; resource IDs and encoding metadata are never serialized. */
export type PreparedObservedWaveformBinding = Readonly<{
  resourceKey: string;
  trackId: string;
  generation: number;
  durationSeconds: number;
  binCount: number;
  scope: "wholeTrack" | "playbackRange";
  range: { startSeconds: number; endSeconds: number } | null;
  sessionKey: string;
  persistenceKey: string | null;
}>;

type StorageLike = Pick<Storage, "getItem" | "setItem" | "removeItem">;
type EncodedBin = [number, number] | null;
type StoredRecord = {
  key: string;
  updatedAt: number;
  expiresAt: number;
  durationSeconds: number;
  binCount: number;
  scope: "wholeTrack" | "playbackRange";
  range: [number, number] | null;
  bins: EncodedBin[];
};
type CacheEnvelope = { schemaVersion: 1; records: StoredRecord[] };
type MemoryRecord = { binding: PreparedObservedWaveformBinding; updatedAt: number; bins: Array<WaveformBin | null> };

const validHash = (value: unknown): value is string => typeof value === "string" && /^[a-f0-9]{64}$/i.test(value);
const validDuration = (value: number) => Number.isFinite(value) && value > 0 && value <= 24 * 60 * 60;
const validCount = (value: number) => Number.isSafeInteger(value) && value > 0 && value <= MAX_WAVEFORM_BIN_COUNT;
const validAmplitude = (bin: unknown): bin is WaveformBin => {
  if (!bin || typeof bin !== "object") return false;
  const { min, max } = bin as WaveformBin;
  return Number.isFinite(min) && Number.isFinite(max) && min >= -1 && max <= 1 && min <= max;
};

function safeLocalStorage(): StorageLike | null {
  try { return typeof localStorage === "undefined" ? null : localStorage; }
  catch { return null; }
}

function validResourceKey(value: string): boolean {
  return typeof value === "string" && value.trim().length > 0 && value.length <= 2048 &&
    !/https?:\/\//i.test(value) && !/\b(?:token|access[_-]?token|authorization|password)\s*[:=]/i.test(value);
}

function validScopeRange(scope: string, range: ObservedWaveformCacheBinding["range"], durationSeconds: number): boolean {
  if (scope === "wholeTrack") return range === null;
  return scope === "playbackRange" && Boolean(range && Number.isFinite(range.startSeconds) && Number.isFinite(range.endSeconds) &&
    range.startSeconds >= 0 && range.endSeconds > range.startSeconds && range.endSeconds <= 86_400 &&
    Math.abs(range.endSeconds - range.startSeconds - durationSeconds) <= 0.05);
}

function rangeForStorage(range: ObservedWaveformCacheBinding["range"]): [number, number] | null {
  return range ? [range.startSeconds, range.endSeconds] : null;
}

/** Validate and compute the persistent digest once, outside the 80 ms observer
 * loop. Unknown stream encodings stay session-only and are scoped to generation. */
export async function prepareObservedWaveformBinding(
  binding: ObservedWaveformCacheBinding,
): Promise<PreparedObservedWaveformBinding | null> {
  if (!validResourceKey(binding.resourceKey) || typeof binding.trackId !== "string" || !binding.trackId || binding.trackId.length > 512 ||
      !Number.isSafeInteger(binding.generation) || binding.generation < 0 || !validDuration(binding.durationSeconds) ||
      !validCount(binding.binCount) || !["wholeTrack", "playbackRange"].includes(binding.scope)) return null;
  const range = binding.range;
  if (!validScopeRange(binding.scope, range, binding.durationSeconds)) return null;

  let stableIdentity: string | null = null;
  let transientIdentity = "unknown";
  if (binding.identity.kind === "stream") {
    if (!/^[A-Za-z0-9_-]{1,64}$/.test(binding.identity.provider) || !/^[A-Za-z0-9:_-]{1,512}$/.test(binding.identity.sourceId)) return null;
    const variant = streamAudioVariantIdentity(binding.identity.audioVariant);
    const transientVariant = binding.identity.audioVariant;
    const boundedLabel = (value: unknown) => typeof value === "string" ? value.slice(0, 128) : null;
    // Even non-durable representations need separate in-memory bindings when
    // their observed encoding changes without a new playback generation.
    transientIdentity = JSON.stringify([binding.identity.provider, binding.identity.sourceId,
      boundedLabel(transientVariant?.formatId), boundedLabel(transientVariant?.codec), boundedLabel(transientVariant?.mimeType),
      typeof transientVariant?.expectedContentLength === "number" && Number.isSafeInteger(transientVariant.expectedContentLength) && transientVariant.expectedContentLength > 0
        ? transientVariant.expectedContentLength : null,
      typeof transientVariant?.containsVideo === "boolean" ? transientVariant.containsVideo : null]);
    // The current Monochrome route has no persistent representation validator.
    // Catalog/source IDs plus an encoding label cannot validate its bytes.
    if (variant && binding.identity.provider.toLowerCase() !== "monochrome") {
      stableIdentity = `stream\n${binding.identity.provider}\n${binding.identity.sourceId}\n${variant}`;
    }
  } else if (binding.identity.kind === "local") {
    if (!/^[A-Za-z0-9:_-]{1,256}$/.test(binding.identity.assetId) || !validHash(binding.identity.verifiedFileHash)) return null;
    stableIdentity = `local\n${binding.identity.assetId}\n${binding.identity.verifiedFileHash.toLowerCase()}`;
  } else if (binding.identity.kind === "localResource") {
    if (!validHash(binding.identity.resourceKeySha256)) return null;
    // This opaque key is computed by the backend from verified local content.
    // Do not include the library asset ID: aliases/remapping preserve identity.
    stableIdentity = `local-content-v1\n${binding.identity.resourceKeySha256.toLowerCase()}`;
  } else if (binding.identity.kind !== "session") return null;

  const rangeText = range ? `${range.startSeconds}\n${range.endSeconds}` : "whole-track";
  const common = `resource=${binding.resourceKey}\ntrack=${binding.trackId}\nduration=${binding.durationSeconds}\nbins=${binding.binCount}\nscope=${binding.scope}\nrange=${rangeText}`;
  const sessionKey = `session\n${binding.generation}\n${common}\nidentity=${stableIdentity ?? transientIdentity}`;
  let persistenceKey: string | null = null;
  if (stableIdentity) {
    try {
      const persistentCommon = binding.identity.kind === "localResource"
        ? `duration=${binding.durationSeconds}\nbins=${binding.binCount}\nscope=${binding.scope}\nrange=${rangeText}`
        : common;
      const bytes = new TextEncoder().encode(`burrata-observed-waveform-cache-v1\n${persistentCommon}\nidentity=${stableIdentity}`);
      const digest = await crypto.subtle.digest("SHA-256", bytes);
      persistenceKey = [...new Uint8Array(digest)].map(byte => byte.toString(16).padStart(2, "0")).join("");
    } catch { persistenceKey = null; }
  }
  return Object.freeze({
    resourceKey: binding.resourceKey, trackId: binding.trackId, generation: binding.generation,
    durationSeconds: binding.durationSeconds, binCount: binding.binCount, scope: binding.scope,
    range: range ? Object.freeze({ ...range }) : null, sessionKey, persistenceKey,
  });
}

function encodeBins(bins: Array<WaveformBin | null>): EncodedBin[] {
  return bins.map(bin => bin ? [bin.min, bin.max] : null);
}

function decodeBins(value: unknown, binCount: number): Array<WaveformBin | null> | null {
  if (!Array.isArray(value) || value.length !== binCount) return null;
  const result: Array<WaveformBin | null> = [];
  for (const item of value) {
    if (item === null) { result.push(null); continue; }
    if (!Array.isArray(item) || item.length !== 2 || !validAmplitude({ min: item[0], max: item[1] })) return null;
    result.push({ min: item[0], max: item[1] });
  }
  return result;
}

function mergeBins(previous: Array<WaveformBin | null>, next: Array<WaveformBin | null>): Array<WaveformBin | null> {
  return next.map((bin, index) => {
    const prior = previous[index];
    if (!prior) return bin ? { ...bin } : null;
    if (!bin) return { ...prior };
    return { min: Math.min(prior.min, bin.min), max: Math.max(prior.max, bin.max) };
  });
}

function validStoredRecord(value: unknown, now: number): value is StoredRecord {
  if (!value || typeof value !== "object") return false;
  const row = value as StoredRecord;
  return validHash(row.key) && Number.isFinite(row.updatedAt) && row.updatedAt <= now + 60_000 &&
    Number.isFinite(row.expiresAt) && row.expiresAt > now && row.expiresAt <= row.updatedAt + OBSERVED_WAVEFORM_CACHE_TTL_MS + 1_000 &&
    validDuration(row.durationSeconds) && validCount(row.binCount) && ["wholeTrack", "playbackRange"].includes(row.scope) &&
    (row.range === null || Array.isArray(row.range) && row.range.length === 2 && row.range.every(Number.isFinite) && row.range[0] >= 0 && row.range[1] > row.range[0]) &&
    validScopeRange(row.scope, row.range ? { startSeconds: row.range[0], endSeconds: row.range[1] } : null, row.durationSeconds) &&
    decodeBins(row.bins, row.binCount) !== null;
}

/** Bounded memory and best-effort durable retention for partial observed bins.
 * Full local/stream analysis is deliberately outside this cache. */
export class ObservedWaveformCache {
  private readonly storage: StorageLike | null;
  private readonly now: () => number;
  private readonly persistent = new Map<string, StoredRecord>();
  private readonly session = new Map<string, MemoryRecord>();
  private sessionApproxBytes = 0;
  private loaded = false;
  private readFailed = false;
  private dirty = false;
  private flushTimer: ReturnType<typeof setTimeout> | null = null;

  constructor(storage: StorageLike | null = safeLocalStorage(), now: () => number = () => Date.now()) {
    this.storage = storage;
    this.now = now;
  }

  restore(binding: PreparedObservedWaveformBinding): WaveformSnapshot | null {
    if (!this.validPrepared(binding)) return null;
    this.load();
    const now = this.now();
    const memory = this.session.get(binding.sessionKey);
    let bins = memory && now - memory.updatedAt >= 0 && now - memory.updatedAt <= OBSERVED_WAVEFORM_CACHE_TTL_MS ? memory.bins : null;
    if (memory && !bins) this.deleteSession(binding.sessionKey);
    if (!bins && binding.persistenceKey) {
      const stored = this.persistent.get(binding.persistenceKey);
      if (stored && stored.expiresAt > now && stored.durationSeconds === binding.durationSeconds && stored.binCount === binding.binCount &&
          stored.scope === binding.scope && JSON.stringify(stored.range) === JSON.stringify(rangeForStorage(binding.range))) {
        bins = decodeBins(stored.bins, binding.binCount);
      }
      else if (stored) { this.persistent.delete(binding.persistenceKey); this.markDirty(); }
    }
    if (!bins || bins.length !== binding.binCount) return null;
    const snapshot = emptyWaveform(binding.generation, binding.trackId, binding.durationSeconds, binding.binCount);
    snapshot.bins = bins.map(bin => bin ? { ...bin } : null);
    snapshot.scope = binding.scope;
    snapshot.complete = false;
    snapshot.source = "observed-playback";
    return snapshot;
  }

  /** Merge new observed bins without filling null gaps. Returns a normalized
   * incomplete snapshot the caller may install after rechecking its owner. */
  record(
    binding: PreparedObservedWaveformBinding,
    snapshot: WaveformSnapshot,
    options: { flushNow?: boolean } = {},
  ): WaveformSnapshot | null {
    if (!this.validPrepared(binding) || snapshot.source !== "observed-playback" || snapshot.complete ||
        snapshot.generation !== binding.generation || snapshot.trackId !== binding.trackId ||
        snapshot.durationSeconds !== binding.durationSeconds || snapshot.bins.length !== binding.binCount ||
        snapshot.scope !== undefined && snapshot.scope !== binding.scope ||
        !snapshot.bins.every(bin => bin === null || validAmplitude(bin))) return null;
    const bins = decodeBins(encodeBins(snapshot.bins), binding.binCount);
    if (!bins) return null;
    this.load();
    const now = this.now();
    const previousMemory = this.session.get(binding.sessionKey);
    let previous = previousMemory && now - previousMemory.updatedAt >= 0 && now - previousMemory.updatedAt <= OBSERVED_WAVEFORM_CACHE_TTL_MS
      ? previousMemory.bins : null;
    if (previousMemory) this.deleteSession(binding.sessionKey);
    if (!previous && binding.persistenceKey) {
      const stored = this.persistent.get(binding.persistenceKey);
      if (stored && stored.expiresAt > now) previous = decodeBins(stored.bins, binding.binCount);
    }
    const merged = previous ? mergeBins(previous, bins) : bins;
    const memoryRecord: MemoryRecord = { binding, updatedAt: now, bins: merged };
    this.session.set(binding.sessionKey, memoryRecord);
    this.sessionApproxBytes += this.sessionRecordBytes(memoryRecord);
    this.evictSession();

    if (binding.persistenceKey && !this.readFailed) {
      const stored = this.persistent.get(binding.persistenceKey);
      const unchanged = stored && stored.expiresAt > now && stored.bins.length === merged.length &&
        merged.every((bin, index) => {
          const previous = stored.bins[index];
          return bin === null ? previous === null : previous !== null && previous[0] === bin.min && previous[1] === bin.max;
        });
      // A paused native observer can return the same snapshot repeatedly.
      // Only changed coverage/amplitudes dirty the durable cache; still retry
      // a previously failed flush when the caller explicitly asks to flush.
      if (!unchanged) {
        this.persistent.set(binding.persistenceKey, {
          key: binding.persistenceKey, updatedAt: now, expiresAt: now + OBSERVED_WAVEFORM_CACHE_TTL_MS,
          durationSeconds: binding.durationSeconds, binCount: binding.binCount, scope: binding.scope,
          range: rangeForStorage(binding.range), bins: encodeBins(merged),
        });
        // Pending records are bounded by entry/bin counts. Enforce the exact
        // serialized byte ceiling at flush, outside the 80 ms observation loop.
        this.trimPersistent(false);
        this.markDirty();
      }
      if (options.flushNow) this.flush();
    }
    const result = emptyWaveform(binding.generation, binding.trackId, binding.durationSeconds, binding.binCount);
    result.bins = merged.map(bin => bin ? { ...bin } : null);
    result.scope = binding.scope;
    result.complete = false;
    return result;
  }

  /** Flush coalesced writes on pause/pagehide or when the caller is closing. */
  flush(): boolean {
    if (!this.dirty) return true;
    if (!this.storage || !this.loaded || this.readFailed) return false;
    this.clearFlushTimer();
    try {
      const envelope: CacheEnvelope = { schemaVersion: 1, records: [...this.persistent.values()] };
      const serialized = JSON.stringify(envelope);
      if (new TextEncoder().encode(serialized).byteLength > OBSERVED_WAVEFORM_CACHE_MAX_SERIALIZED_BYTES) {
        this.trimPersistent();
        return this.flush();
      }
      this.storage.setItem(OBSERVED_WAVEFORM_CACHE_STORAGE_KEY, serialized);
      this.dirty = false;
      return true;
    } catch {
      this.scheduleFlush();
      return false;
    }
  }

  get memoryEntryCount(): number { return this.session.size; }
  get memoryApproxBytes(): number { return this.sessionApproxBytes; }
  get persistedEntryCount(): number { return this.persistent.size; }

  private validPrepared(binding: PreparedObservedWaveformBinding): boolean {
    return Boolean(binding && validResourceKey(binding.resourceKey) && typeof binding.sessionKey === "string" &&
      binding.sessionKey.length <= 8192 && (!binding.persistenceKey || validHash(binding.persistenceKey)) &&
      typeof binding.trackId === "string" && binding.trackId.length > 0 && binding.trackId.length <= 512 &&
      Number.isSafeInteger(binding.generation) && binding.generation >= 0 && validDuration(binding.durationSeconds) &&
      validCount(binding.binCount) && validScopeRange(binding.scope, binding.range, binding.durationSeconds));
  }

  private load(): void {
    if (this.loaded || !this.storage) return;
    this.readFailed = false;
    let raw: string | null;
    try { raw = this.storage.getItem(OBSERVED_WAVEFORM_CACHE_STORAGE_KEY); }
    catch { this.readFailed = true; return; }
    this.loaded = true;
    if (raw === null) return;
    if (new TextEncoder().encode(raw).byteLength > OBSERVED_WAVEFORM_CACHE_MAX_SERIALIZED_BYTES) {
      try { this.storage.removeItem(OBSERVED_WAVEFORM_CACHE_STORAGE_KEY); } catch { this.readFailed = true; }
      return;
    }
    let parsed: unknown;
    try { parsed = JSON.parse(raw); }
    catch {
      try { this.storage.removeItem(OBSERVED_WAVEFORM_CACHE_STORAGE_KEY); } catch { this.readFailed = true; }
      this.dirty = false;
      return;
    }
    if (!parsed || typeof parsed !== "object" || (parsed as CacheEnvelope).schemaVersion !== 1 || !Array.isArray((parsed as CacheEnvelope).records)) {
      try { this.storage.removeItem(OBSERVED_WAVEFORM_CACHE_STORAGE_KEY); } catch { this.readFailed = true; }
      return;
    }
    const now = this.now();
    for (const row of (parsed as CacheEnvelope).records) {
      if (!validStoredRecord(row, now)) { this.dirty = true; continue; }
      this.persistent.set(row.key, row);
    }
    this.trimPersistent();
  }

  private markDirty(): void {
    if (!this.storage || this.readFailed) return;
    this.dirty = true;
    this.scheduleFlush();
  }

  private scheduleFlush(): void {
    if (!this.dirty || this.flushTimer !== null) return;
    this.flushTimer = setTimeout(() => { this.flushTimer = null; this.flush(); }, OBSERVED_WAVEFORM_CACHE_WRITE_DELAY_MS);
  }

  private clearFlushTimer(): void {
    if (this.flushTimer !== null) clearTimeout(this.flushTimer);
    this.flushTimer = null;
  }

  private evictSession(): void {
    while (this.session.size > OBSERVED_WAVEFORM_CACHE_MAX_ENTRIES || this.sessionApproxBytes > OBSERVED_WAVEFORM_CACHE_MAX_SESSION_BYTES) {
      const oldest = [...this.session.entries()].sort((a, b) => a[1].updatedAt - b[1].updatedAt)[0];
      if (!oldest) break;
      this.deleteSession(oldest[0]);
    }
  }

  private sessionRecordBytes(record: MemoryRecord): number {
    // Conservative bounded estimate for the JS arrays/objects and their exact
    // resource key, which remains memory-only.
    return 256 + record.bins.length * 48 + record.binding.resourceKey.length * 2 + record.binding.trackId.length * 2;
  }

  private deleteSession(key: string): void {
    const previous = this.session.get(key);
    if (!previous) return;
    this.sessionApproxBytes = Math.max(0, this.sessionApproxBytes - this.sessionRecordBytes(previous));
    this.session.delete(key);
  }

  private trimPersistent(checkSerializedBytes = true): void {
    const serializedBytes = () => new TextEncoder().encode(JSON.stringify({ schemaVersion: 1, records: [...this.persistent.values()] })).byteLength;
    while (this.persistent.size > OBSERVED_WAVEFORM_CACHE_MAX_ENTRIES || checkSerializedBytes && serializedBytes() > OBSERVED_WAVEFORM_CACHE_MAX_SERIALIZED_BYTES) {
      const oldest = [...this.persistent.entries()].sort((a, b) => a[1].updatedAt - b[1].updatedAt)[0];
      if (!oldest) break;
      this.persistent.delete(oldest[0]);
      this.dirty = true;
    }
  }
}
