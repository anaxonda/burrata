import { describe, expect, it } from "vitest";
import { createWebviewMediaEventRecorder } from "./webview-media-events";

const owner = { sessionId: "session-a", sourceGeneration: 4, entryId: "entry-a", seekSerial: 2 };
function media(currentTime = 3) {
  return { readyState: 2, networkState: 2, paused: false, currentTime, buffered: { length: 2, start: (i: number) => i ? 5 : 0, end: (i: number) => i ? 8 : 4 } };
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
    expect(result.events[0]).toEqual({ atMs: 2, type: "waiting", readyState: 2, networkState: 2, paused: false, currentTime: 3, buffered: [{ start: 0, end: 4 }, { start: 5, end: 8 }] });
    expect(Object.keys(result.owner).sort()).toEqual(["entryId", "seekSerial", "sessionId", "sourceGeneration"]);
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
});
