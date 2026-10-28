import { describe, expect, it, vi } from "vitest";
import { createBackupCheckpointResponse, respondToBackupCheckpoint } from "./backup-checkpoint";

describe("backup checkpoint response", () => {
  it("returns fresh frontend preferences for scheduled backup requests without playback override", () => {
    const response = createBackupCheckpointResponse(
      { requestId: "scheduled-1", includeWebviewPlayback: false, deadlineMs: 8_000 },
      { "burrata.waveform-enabled.v1": true, "burrata.discovery.search-history.v1": { version: 1, entries: [{ query: "ambient" }] } },
      { queue: { tracks: [{ id: "stale" }] } },
    );
    expect(response).toEqual({
      requestId: "scheduled-1",
      frontendPreferences: {
        "burrata.waveform-enabled.v1": true,
        "burrata.discovery.search-history.v1": { version: 1, entries: [{ query: "ambient" }] },
      },
    });
    expect(response.webviewPlayback).toBeUndefined();
  });

  it("mock coordinator receives scheduled checkpoint response without reading WebView playback", async () => {
    const submit = vi.fn(async () => undefined);
    const capturePlayback = vi.fn(() => ({ queue: { tracks: [{ id: "stale-webview" }] } }));
    const response = await respondToBackupCheckpoint(
      { requestId: "scheduled-coordinator-1", includeWebviewPlayback: false, deadlineMs: 8_000 },
      () => ({ "burrata.waveform-enabled.v1": true }),
      capturePlayback,
      submit,
    );
    expect(capturePlayback).not.toHaveBeenCalled();
    expect(submit).toHaveBeenCalledWith({
      requestId: "scheduled-coordinator-1",
      frontendPreferences: { "burrata.waveform-enabled.v1": true },
    });
    expect(response.webviewPlayback).toBeUndefined();
  });

  it("pauses before capturing restore-purpose playback and preserves occurrence state", async () => {
    const order: string[] = [];
    const pause = vi.fn(async () => { order.push("paused"); });
    const submit = vi.fn(async () => { order.push("submitted"); });
    const response = await respondToBackupCheckpoint(
      { requestId: "restore-1", includeWebviewPlayback: true, restorePurpose: true, deadlineMs: 8_000 },
      () => { order.push("preferences"); return { theme: "dark" }; },
      () => { order.push("playback"); return { queue: { entryIds: ["occurrence-a", "occurrence-b"], currentIndex: 1 }, session: { position: 12.25 } }; },
      submit,
      pause,
    );
    expect(order).toEqual(["paused", "preferences", "playback", "submitted"]);
    expect(response.webviewPlayback).toEqual({
      queue: { entryIds: ["occurrence-a", "occurrence-b"], currentIndex: 1 },
      session: { position: 12.25 },
    });
  });

  it("does not submit a restore checkpoint when playback cannot be paused", async () => {
    const submit = vi.fn(async () => undefined);
    await expect(respondToBackupCheckpoint(
      { requestId: "restore-failed", includeWebviewPlayback: false, restorePurpose: true, deadlineMs: 8_000 },
      () => ({}),
      () => ({}),
      submit,
      async () => { throw new Error("pause failed"); },
    )).rejects.toThrow("pause failed");
    expect(submit).not.toHaveBeenCalled();
  });

  it("keeps the complete WebView occurrence queue above the old native page size", () => {
    const tracks = Array.from({ length: 257 }, (_, index) => ({ id: `listing-${index}` }));
    const entryIds = tracks.map((_, index) => `entry-${index}`);
    const response = createBackupCheckpointResponse(
      { requestId: "webview-1", includeWebviewPlayback: true, deadlineMs: 8_000 },
      { "burrata.playlist.include-ytdlp-search.v1": true },
      {
        queue: { tracks, entryIds, currentIndex: 201, repeat: "all" },
        session: { index: 201, position: 34.5, repeat: "all", shuffle: false, volume: 0.7, queueRevision: 23 },
      },
    );
    const checkpointQueue = (response.webviewPlayback?.queue as { tracks: unknown[]; entryIds: unknown[]; currentIndex: number });
    expect(checkpointQueue.tracks).toHaveLength(257);
    expect(checkpointQueue.entryIds).toHaveLength(257);
    expect(checkpointQueue.currentIndex).toBe(201);
    expect(response.frontendPreferences["burrata.playlist.include-ytdlp-search.v1"]).toBe(true);
  });

  it("rejects invalid or incomplete checkpoint requests", () => {
    expect(() => createBackupCheckpointResponse(
      { requestId: "bad\nrequest", includeWebviewPlayback: false, deadlineMs: 1000 }, {},
    )).toThrow("Invalid backup checkpoint request ID");
    expect(() => createBackupCheckpointResponse(
      { requestId: "missing-playback", includeWebviewPlayback: true, deadlineMs: 1000 }, {},
    )).toThrow("The requested WebView playback checkpoint is unavailable");
  });
});
