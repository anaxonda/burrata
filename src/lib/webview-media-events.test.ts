import { describe, expect, it } from "vitest";
import { createWebviewMediaEventRecorder } from "./webview-media-events";

const owner = { sessionId: "session-a", sourceGeneration: 4, entryId: "entry-a", seekSerial: 2 };
function media(currentTime = 3) {
  return { readyState: 2, networkState: 2, paused: false, currentTime, duration: 12.5, buffered: { length: 2, start: (i: number) => i ? 5 : 0, end: (i: number) => i ? 8 : 4 } };
}

describe("WebView media event recorder", () => {
  it("bounds event history and captures only safe media fields", () => {
    let clock = 0;
    const recorder = createWebviewMediaEventRecorder({ enabled: true, now: () => ++clock, capacity: 3 });
    for (const type of ["play", "waiting", "canplay", "playing"]) recorder.record(owner, type, media());
    const result = recorder.snapshot(owner);
    expect(result).not.toBeNull();
    if (!result) throw new Error("Expected event snapshot");
    expect(result.events.map(event => event.type)).toEqual(["waiting", "canplay", "playing"]);
    expect(result.events[0]).toEqual({ atMs: 2, type: "waiting", readyState: 2, networkState: 2, paused: false, currentTime: 3, durationSeconds: 12.5, webviewGeneration: 4, buffered: [{ start: 0, end: 4 }, { start: 5, end: 8 }] });
    expect(Object.keys(result.owner).sort()).toEqual(["entryId", "seekSerial", "sessionId", "sourceGeneration"]);
  });

  it("records a bounded watchdog expiry sample", () => {
    const recorder = createWebviewMediaEventRecorder({ enabled: true, now: () => 20_000 });
    recorder.record(owner, "watchdog-expired", media(3));
    expect(recorder.snapshotHistory()).toMatchObject([{ type: "watchdog-expired", currentTime: 3, readyState: 2, buffered: [{ start: 0, end: 4 }, { start: 5, end: 8 }] }]);
  });

  it("resets on owner change and prevents stale snapshots from restoring old history", () => {
    const recorder = createWebviewMediaEventRecorder({ enabled: true });
    recorder.record(owner, "playing", media());
    const next = { ...owner, entryId: "entry-b", seekSerial: 3 };
    expect(recorder.snapshot(next)).toBeNull();
    expect(recorder.snapshot(owner)).toBeNull();
    recorder.record(next, "seeking", media(20));
    const result = recorder.snapshot(next);
    expect(result).not.toBeNull();
    if (!result) throw new Error("Expected event snapshot");
    expect(result.events.map(event => event.type)).toEqual(["seeking"]);
    expect(recorder.snapshot(owner)).toBeNull();
  });

  it("does not capture while disabled or for unapproved event types", () => {
    const disabled = createWebviewMediaEventRecorder({ enabled: false });
    disabled.record(owner, "playing", media());
    expect(disabled.snapshot(owner)).toBeNull();
    const enabled = createWebviewMediaEventRecorder({ enabled: true });
    enabled.record(owner, "timeupdate", media());
    expect(enabled.snapshot(owner)).toBeNull();
  });

  it("clears all owner and sample data on reset", () => {
    const recorder = createWebviewMediaEventRecorder({ enabled: true });
    recorder.record(owner, "seeked", media());
    recorder.reset();
    expect(recorder.snapshot(owner)).toBeNull();
  });

  it("can follow the diagnostics setting and clears samples when disabled", () => {
    const recorder = createWebviewMediaEventRecorder();
    recorder.record(owner, "playing", media());
    expect(recorder.snapshot(owner)).toBeNull();
    recorder.setEnabled(true);
    recorder.record(owner, "waiting", media());
    expect(recorder.snapshot(owner)?.events).toHaveLength(1);
    recorder.setEnabled(false);
    expect(recorder.enabled).toBe(false);
    expect(recorder.snapshot(owner)).toBeNull();
    recorder.setEnabled(true);
    expect(recorder.snapshotLatest()?.map(event=>event.type)).toEqual(["waiting"]);
    recorder.reset();
    expect(recorder.snapshotLatest()).toBeNull();
  });

  it("records only the numeric media error code", () => {
    const recorder = createWebviewMediaEventRecorder({enabled:true});
    recorder.record(owner, "error", {...media(), error:{code:3, message:"https://private.invalid/token"} as MediaError});
    expect(recorder.snapshot(owner)?.events[0]).toMatchObject({type:"error",mediaErrorCode:3});
    expect(JSON.stringify(recorder.snapshot(owner))).not.toContain("private.invalid");
  });

  it("retains finite bounded media durations and rejects unknown or oversized durations", () => {
    const recorder = createWebviewMediaEventRecorder({enabled:true});
    recorder.record(owner, "playing", {...media(), duration: 120.093});
    recorder.record({...owner, seekSerial:3}, "waiting", {...media(), duration: Infinity});
    recorder.record({...owner, seekSerial:4}, "waiting", {...media(), duration: NaN});
    recorder.record({...owner, seekSerial:5}, "waiting", {...media(), duration: 86_400.1});
    expect(recorder.snapshotHistory().map(event => event.durationSeconds)).toEqual([120.093, null, null, null]);
  });

  it("retains a safe source generation on each event for playback trace correlation", () => {
    const recorder = createWebviewMediaEventRecorder({enabled:true});
    const sourceGenerations = [52, -1, Number.MAX_SAFE_INTEGER + 1, NaN];
    sourceGenerations.forEach((sourceGeneration, seekSerial) => recorder.record(
      {...owner, sourceGeneration, seekSerial}, "playing", media()));
    expect(recorder.snapshotHistory().map(event => event.webviewGeneration)).toEqual([52, null, null, null]);
  });

  it("can retain a sanitized recent-event view after the UI clears the failed source", () => {
    const recorder = createWebviewMediaEventRecorder({enabled:true});
    recorder.record(owner, "error", media());
    expect(recorder.snapshotLatest()?.map(event => event.type)).toEqual(["error"]);
    recorder.reset();
    expect(recorder.snapshotLatest()).toBeNull();
  });

  it("keeps failed-attempt events across a successful retry until explicit clear", () => {
    const recorder = createWebviewMediaEventRecorder({enabled:true});
    expect(recorder.beginAttempt()).toBe(1);
    recorder.record(owner, "error", {...media(), error:{code:3} as MediaError});
    expect(recorder.beginAttempt()).toBe(2);
    recorder.record({...owner, sourceGeneration:5}, "playing", media());
    recorder.setEnabled(false);
    expect(recorder.snapshotHistory().map(event=>[event.attempt,event.type])).toEqual([[1,"error"],[2,"playing"]]);
    recorder.setEnabled(true);
    expect(recorder.snapshotHistory()).toHaveLength(2);
    recorder.reset();
    expect(recorder.snapshotHistory()).toEqual([]);
  });
});
