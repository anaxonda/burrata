import { describe, expect, it } from "vitest";
import { canRequestWebviewLiveBpm, parseWebviewLiveBpmResponse, supportsWebviewLiveBpmRoute, WEBVIEW_LIVE_BPM_SAMPLE_RATE, WebviewLiveBpmWindow } from "./webview-live-bpm";

const owner = { trackId: "track-a", generation: 4, seekSerial: 0 };

describe("WebView live BPM PCM window", () => {
  it("accepts only renderer routes known to expose supported decoded audio", () => {
    for (const kind of ["audio", "direct", "local"]) expect(supportsWebviewLiveBpmRoute(kind, "same-origin-relay")).toBe(true);
    expect(supportsWebviewLiveBpmRoute("direct", "unsupported")).toBe(false);
    expect(supportsWebviewLiveBpmRoute("video", "same-origin-relay")).toBe(false);
  });

  it("waits for eight seconds and retains no more than twelve seconds", () => {
    const window = new WebviewLiveBpmWindow(owner);
    const oneSecond = new Float32Array(WEBVIEW_LIVE_BPM_SAMPLE_RATE).fill(0.25);
    for (let i = 0; i < 7; i++) {
      expect(window.append(oneSecond)).toBe(true);
      expect(window.snapshot()).toBeNull();
    }
    for (let i = 0; i < 7; i++) expect(window.append(oneSecond)).toBe(true);
    expect(window.seconds).toBe(12);
    const result = window.snapshot();
    expect(result).toHaveLength(12 * WEBVIEW_LIVE_BPM_SAMPLE_RATE);
    expect(result?.every(sample => sample === 0.25)).toBe(true);
    result?.fill(0);
    expect(window.snapshot()?.[0]).toBe(0.25);
  });

  it("clears the window after malformed PCM so stale audio cannot be estimated", () => {
    const window = new WebviewLiveBpmWindow(owner);
    const oneSecond = new Float32Array(WEBVIEW_LIVE_BPM_SAMPLE_RATE).fill(0.1);
    for (let i = 0; i < 8; i++) expect(window.append(oneSecond)).toBe(true);
    expect(window.snapshot()).not.toBeNull();
    expect(window.append(new Float32Array([Number.NaN]))).toBe(false);
    expect(window.seconds).toBe(0);
    expect(window.snapshot()).toBeNull();
  });

  it("rejects empty and oversized one-second observer batches", () => {
    const window = new WebviewLiveBpmWindow(owner);
    expect(window.append(new Float32Array())).toBe(false);
    expect(window.append(new Float32Array(WEBVIEW_LIVE_BPM_SAMPLE_RATE + 1))).toBe(false);
  });

  it("clears samples when track, generation, or seek ownership changes", () => {
    const window = new WebviewLiveBpmWindow(owner);
    const oneSecond = new Float32Array(WEBVIEW_LIVE_BPM_SAMPLE_RATE).fill(0.2);
    for (let i = 0; i < 8; i++) window.append(oneSecond, owner);
    expect(window.snapshot(true, owner)).not.toBeNull();
    const nextOwner = { ...owner, trackId: "track-b", generation: 5 };
    window.bind(nextOwner);
    expect(window.snapshot(true, nextOwner)).toBeNull();
    window.append(oneSecond, nextOwner);
    window.bind({ ...nextOwner, seekSerial: 1 });
    expect(window.seconds).toBe(0);
  });

  it("does not expose a request window after disable or for a stale owner", () => {
    const window = new WebviewLiveBpmWindow(owner);
    const oneSecond = new Float32Array(WEBVIEW_LIVE_BPM_SAMPLE_RATE).fill(0.2);
    for (let i = 0; i < 8; i++) window.append(oneSecond, owner);
    expect(canRequestWebviewLiveBpm(true, true, owner, owner, window.seconds)).toBe(true);
    expect(canRequestWebviewLiveBpm(false, true, owner, owner, window.seconds)).toBe(false);
    expect(window.snapshot(false, owner)).toBeNull();
    expect(window.seconds).toBe(0);
    expect(canRequestWebviewLiveBpm(true, true, owner, { ...owner, generation: 3 }, 8)).toBe(false);
    expect(canRequestWebviewLiveBpm(true, false, owner, owner, 8)).toBe(false);
  });

  it("distinguishes estimator no-estimate from both ownership rejection outcomes", () => {
    expect(parseWebviewLiveBpmResponse(null).status).toBe("no_estimate");
    expect(parseWebviewLiveBpmResponse({ status: "no_estimate", estimate: null }).status).toBe("no_estimate");
    expect(parseWebviewLiveBpmResponse({ status: "owner_mismatch_before" }).status).toBe("owner_mismatch_before");
    expect(parseWebviewLiveBpmResponse({ status: "owner_changed_after" }).status).toBe("owner_changed_after");
    expect(parseWebviewLiveBpmResponse({ status: "success", estimate: { bpm: 120 } })).toEqual({ status: "success", estimate: { bpm: 120 }, keyEstimate: null });
    expect(parseWebviewLiveBpmResponse({ status: "success", estimate: null, keyEstimate: { key: "Am" } })).toEqual({ status: "success", estimate: null, keyEstimate: { key: "Am" } });
  });

  it("keeps bounded per-owner capture and response counters, then resets them on owner change", () => {
    let now = 100;
    const window = new WebviewLiveBpmWindow(owner, () => now);
    const oneSecond = new Float32Array(WEBVIEW_LIVE_BPM_SAMPLE_RATE).fill(0.1);
    now += 375;
    window.markTapAttached();
    now += 250;
    expect(window.append(oneSecond)).toBe(true);
    expect(window.diagnosticSnapshot).toMatchObject({ tapAttached: true, firstBatchElapsedMs: 250 });
    for (let i = 0; i < 7; i++) window.append(oneSecond);
    window.recordAttempt();
    window.recordOutcome("no_estimate", 37.4);
    window.recordAttempt();
    window.recordOutcome("owner_mismatch_before", 5);
    expect(window.diagnosticSnapshot).toMatchObject({ pcmSeconds: 8, batchCount: 8, attempts: 2, nullResponses: 1,
      ownerMismatchBefore: 1, ownerChangedAfter: 0, lastOutcome: "owner_mismatch_before", lastResponseMs: 5 });
    window.bind({ ...owner, trackId: "track-b", generation: 5 });
    expect(window.diagnosticSnapshot).toMatchObject({ pcmSeconds: 0, batchCount: 0, attempts: 0, nullResponses: 0,
      resetCount: 1, lastResetReason: "owner_changed" });
  });

  it("records a fixed capture-gap reset reason without retaining previous PCM", () => {
    const window = new WebviewLiveBpmWindow(owner);
    window.append(new Float32Array(WEBVIEW_LIVE_BPM_SAMPLE_RATE).fill(0.1));
    window.reset("capture_gap");
    expect(window.seconds).toBe(0);
    expect(window.diagnosticSnapshot).toMatchObject({ pcmSeconds: 0, resetCount: 1, lastResetReason: "capture_gap" });
  });
});
