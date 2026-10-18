import { describe, expect, it } from "vitest";
import { WebviewBufferingRecovery, type WebviewProgressOwner } from "./webview-buffering-recovery";

describe("WebView buffering UI recovery", () => {
  it("requires repeated plausible clock progress from the current readyState-2 owner", () => {
    const recovery = new WebviewBufferingRecovery();
    const node = {};
    const owner: WebviewProgressOwner = { node, sourceGeneration: 4, seekSerial: 2 };
    const progress = (mediaTime: number, wallTimeMs: number) => recovery.observe({ owner, currentNode: node, mediaTime, wallTimeMs, paused: false, seeking: false, ended: false, hasError: false });

    expect(progress(48, 0)).toBe(false);
    expect(progress(48.5, 500)).toBe(false);
    expect(progress(49, 1000)).toBe(true);
  });

  it("does not recover on a seek jump or true halted clock", () => {
    const recovery = new WebviewBufferingRecovery();
    const node = {};
    const owner: WebviewProgressOwner = { node, sourceGeneration: 4, seekSerial: 2 };
    const progress = (mediaTime: number, wallTimeMs: number) => recovery.observe({ owner, currentNode: node, mediaTime, wallTimeMs, paused: false, seeking: false, ended: false, hasError: false });

    expect(progress(10, 0)).toBe(false);
    expect(progress(80, 500)).toBe(false);
    expect(progress(80, 1000)).toBe(false);
    expect(progress(80, 1500)).toBe(false);
    expect(progress(80.5, 2000)).toBe(false);
    expect(progress(81, 2500)).toBe(true);
  });

  it("rejects implausible clock rates from jumps and very slow stutter", () => {
    const recovery = new WebviewBufferingRecovery();
    const node = {};
    const owner: WebviewProgressOwner = { node, sourceGeneration: 4, seekSerial: 2 };
    const progress = (mediaTime: number, wallTimeMs: number) => recovery.observe({ owner, currentNode: node, mediaTime, wallTimeMs, paused: false, seeking: false, ended: false, hasError: false });

    expect(progress(0, 0)).toBe(false);
    expect(progress(10, 500)).toBe(false);
    expect(progress(10.5, 1000)).toBe(false);
    expect(progress(11, 1500)).toBe(true);
    recovery.reset();
    expect(progress(0, 0)).toBe(false);
    expect(progress(0.1, 2500)).toBe(false);
    expect(progress(0.2, 5000)).toBe(false);
    expect(progress(0.3, 7500)).toBe(false);
  });

  it("ignores paused, seeking, errored, and stale-node timeupdates", () => {
    const recovery = new WebviewBufferingRecovery();
    const node = {}, staleNode = {};
    const owner: WebviewProgressOwner = { node, sourceGeneration: 4, seekSerial: 2 };
    const observe = (overrides: Partial<Parameters<typeof recovery.observe>[0]> = {}) => recovery.observe({
      owner, currentNode: node, mediaTime: 1, wallTimeMs: 0, paused: false, seeking: false, ended: false, hasError: false, ...overrides,
    });

    expect(observe({ paused: true })).toBe(false);
    expect(observe({ mediaTime: 1, wallTimeMs: 100 })).toBe(false);
    expect(observe({ currentNode: staleNode, mediaTime: 2, wallTimeMs: 600 })).toBe(false);
    expect(observe({ seeking: true, mediaTime: 2, wallTimeMs: 600 })).toBe(false);
    expect(observe({ mediaTime: 2, wallTimeMs: 700 })).toBe(false);
    expect(observe({ hasError: true, mediaTime: 2.5, wallTimeMs: 1200 })).toBe(false);
    expect(observe({ mediaTime: 3, wallTimeMs: 1700 })).toBe(false);
    expect(observe({ mediaTime: 3.5, wallTimeMs: 2200 })).toBe(false);
    expect(observe({ mediaTime: 4, wallTimeMs: 2700 })).toBe(true);
  });
});
