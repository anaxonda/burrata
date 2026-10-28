import type { LiveKeyEstimate } from "./types";

export const WEBVIEW_LIVE_BPM_SAMPLE_RATE = 22_050;
export const WEBVIEW_LIVE_BPM_MIN_SECONDS = 8;
export const WEBVIEW_LIVE_BPM_MAX_SECONDS = 12;
const MAX_SAMPLES = WEBVIEW_LIVE_BPM_SAMPLE_RATE * WEBVIEW_LIVE_BPM_MAX_SECONDS;

export type WebviewLiveBpmOwner = { trackId: string; generation: number; seekSerial: number };
export type WebviewLiveBpmOutcomeCode = "success" | "no_estimate" | "owner_mismatch_before" | "owner_changed_after" | "disabled" | "native_active" | "error";
export type WebviewLiveBpmDiagnostics = {
  tapAttached: boolean; pcmSeconds: number; batchCount: number; firstBatchElapsedMs: number | null;
  attempts: number; nullResponses: number; ownerMismatchBefore: number; ownerChangedAfter: number;
  lastOutcome: WebviewLiveBpmOutcomeCode | null; lastResponseMs: number | null;
  resetCount: number; lastResetReason: "owner_changed" | "capture_gap" | "invalid_batch" | "disabled" | null;
};

const COUNTER_MAX = 65_535;
const boundedIncrement = (value: number) => Math.min(COUNTER_MAX, value + 1);

export function supportsWebviewLiveBpmRoute(kind: string | null | undefined, route: string | null | undefined): boolean {
  return (kind === "audio" || kind === "direct" || kind === "local") && route === "same-origin-relay";
}

function sameOwner(a: WebviewLiveBpmOwner | null, b: WebviewLiveBpmOwner | null): boolean {
  return Boolean(a && b && a.trackId === b.trackId && a.generation === b.generation && a.seekSerial === b.seekSerial);
}

/** A small owner-bound rolling window. Gaps and lifecycle changes require reset(). */
export class WebviewLiveBpmWindow {
  private samples = new Float32Array(0);
  private owner: WebviewLiveBpmOwner | null;
  private readonly now: () => number;
  private boundAt: number;
  private diagnostics: WebviewLiveBpmDiagnostics;

  constructor(owner: WebviewLiveBpmOwner | null = null, now: () => number = () => performance.now()) {
    this.owner = owner ? { ...owner } : null;
    this.now = now;
    this.boundAt = now();
    this.diagnostics = this.emptyDiagnostics();
  }

  private emptyDiagnostics(): WebviewLiveBpmDiagnostics {
    return { tapAttached: false, pcmSeconds: 0, batchCount: 0, firstBatchElapsedMs: null, attempts: 0, nullResponses: 0,
      ownerMismatchBefore: 0, ownerChangedAfter: 0, lastOutcome: null, lastResponseMs: null,
      resetCount: 0, lastResetReason: null };
  }

  get diagnosticSnapshot(): WebviewLiveBpmDiagnostics {
    return { ...this.diagnostics, pcmSeconds: Number(this.seconds.toFixed(2)) };
  }

  recordAttempt(): void { this.diagnostics.attempts = boundedIncrement(this.diagnostics.attempts); }
  markTapAttached(): void { this.diagnostics.tapAttached = true; this.boundAt = this.now(); }

  recordOutcome(code: WebviewLiveBpmOutcomeCode, responseMs: number): void {
    this.diagnostics.lastOutcome = code;
    this.diagnostics.lastResponseMs = Number.isFinite(responseMs) ? Math.max(0, Math.min(120_000, Math.round(responseMs))) : null;
    if (code === "no_estimate") this.diagnostics.nullResponses = boundedIncrement(this.diagnostics.nullResponses);
    if (code === "owner_mismatch_before") this.diagnostics.ownerMismatchBefore = boundedIncrement(this.diagnostics.ownerMismatchBefore);
    if (code === "owner_changed_after") this.diagnostics.ownerChangedAfter = boundedIncrement(this.diagnostics.ownerChangedAfter);
  }

  bind(owner: WebviewLiveBpmOwner | null): void {
    if (!sameOwner(this.owner, owner)) {
      const nextResetCount = boundedIncrement(this.diagnostics.resetCount);
      this.samples = new Float32Array(0);
      this.diagnostics = this.emptyDiagnostics();
      this.diagnostics.resetCount = nextResetCount;
      this.diagnostics.lastResetReason = "owner_changed";
    }
    this.owner = owner ? { ...owner } : null;
    this.boundAt = this.now();
  }

  append(batch: Float32Array, owner = this.owner): boolean {
    if (!sameOwner(this.owner, owner)) return false;
    if (!batch.length || batch.length > WEBVIEW_LIVE_BPM_SAMPLE_RATE ||
        Array.from(batch).some(sample => !Number.isFinite(sample))) {
      this.reset("invalid_batch");
      return false;
    }
    const keep = Math.min(this.samples.length, MAX_SAMPLES - batch.length);
    const next = new Float32Array(keep + batch.length);
    if (keep) next.set(this.samples.subarray(this.samples.length - keep));
    next.set(batch, keep);
    this.samples = next;
    if (this.diagnostics.batchCount === 0) this.diagnostics.firstBatchElapsedMs = Math.max(0, Math.round(this.now() - this.boundAt));
    this.diagnostics.batchCount = boundedIncrement(this.diagnostics.batchCount);
    this.diagnostics.pcmSeconds = Number(this.seconds.toFixed(2));
    return true;
  }

  get seconds(): number { return this.samples.length / WEBVIEW_LIVE_BPM_SAMPLE_RATE; }

  /** Return a copy so callers cannot mutate the retained PCM window. */
  snapshot(enabled = true, owner = this.owner, minSeconds = WEBVIEW_LIVE_BPM_MIN_SECONDS): Float32Array | null {
    if (!enabled) { this.reset("disabled"); return null; }
    return sameOwner(this.owner, owner) && this.seconds >= minSeconds ? this.samples.slice() : null;
  }

  reset(reason: WebviewLiveBpmDiagnostics["lastResetReason"] = null): void {
    this.samples = new Float32Array(0);
    if (reason) {
      this.diagnostics.resetCount = boundedIncrement(this.diagnostics.resetCount);
      this.diagnostics.lastResetReason = reason;
    }
    this.diagnostics.pcmSeconds = 0;
    this.diagnostics.batchCount = 0;
    this.diagnostics.firstBatchElapsedMs = null;
    this.boundAt = this.now();
  }
}

export type WebviewLiveBpmApiResponse<T> = { status: WebviewLiveBpmOutcomeCode; estimate?: T | null; keyEstimate?: LiveKeyEstimate | null };

/** Accept the bounded typed response and legacy estimate/null responses during upgrades. */
export function parseWebviewLiveBpmResponse<T>(value: unknown): WebviewLiveBpmApiResponse<T> {
  if (value === null) return { status: "no_estimate", estimate: null };
  if (!value || typeof value !== "object") return { status: "error", estimate: null };
  const record = value as Record<string, unknown>;
  if (typeof record.status === "string" && ["success", "no_estimate", "owner_mismatch_before", "owner_changed_after", "disabled", "native_active", "error"].includes(record.status)) {
    return {
      status: record.status as WebviewLiveBpmOutcomeCode,
      estimate: (record.estimate as T | null | undefined) ?? null,
      keyEstimate: (record.keyEstimate as LiveKeyEstimate | null | undefined) ?? null,
    };
  }
  return { status: "success", estimate: value as T };
}

/** Prevent disabled, paused, or stale-owner PCM from triggering backend work. */
export function canRequestWebviewLiveBpm(
  enabled: boolean,
  playing: boolean,
  requestedOwner: WebviewLiveBpmOwner,
  currentOwner: WebviewLiveBpmOwner | null,
  windowSeconds: number,
): boolean {
  return enabled && playing && sameOwner(requestedOwner, currentOwner) &&
    Number.isFinite(windowSeconds) && windowSeconds >= WEBVIEW_LIVE_BPM_MIN_SECONDS &&
    windowSeconds <= WEBVIEW_LIVE_BPM_MAX_SECONDS;
}
