import { describe, it, expect, vi } from "vitest";
import { dispatchQueuedPlayback, exactQueuedTracks, type QueuedPlayDependencies, type QueuedPlayOutcome } from "./queued-playback";
function deferred<T>() { let resolve!: (value: T) => void, reject!: (error: unknown) => void; const promise = new Promise<T>((a, b) => { resolve = a; reject = b; }); return {
  promise, resolve, reject
}; }
const request = {
  operationId: "owner", requestId: "rpc", trackIds: ["a", "a", "b"]
};
function setup() { const task = deferred<QueuedPlayOutcome>(); let current = true; const deps: QueuedPlayDependencies<{
  id: string;
}> = {
  load: vi.fn(async () => [{ id: "b" }, { id: "a" }, { id: "a" }]), current: () => current, prepare: vi.fn((tracks: {
    id: string;
  }[]) => ({ receipt: {
      engine: "webview" as const, trackIds: tracks.map(t => t.id), queueRevision: "q", queueEntryIds: ["a-1", "a-2", "b-1"], acceptedGeneration: 1
    }, completion: task.promise })), accept: vi.fn(async () => ({ accepted: true })), lookup: vi.fn(async () => ({ status: "accepted", receipt: {} })), stopOwned: vi.fn(), update: vi.fn(async () => ({ accepted: true }))
}; return {
  deps, task, replace: () => { current = false; }
}; }
const pulse = async () => { await Promise.resolve(); await Promise.resolve(); await Promise.resolve(); };
describe("queued playback operation dispatch", () => {
  it("accepts exact duplicate occurrences before resolver completion, then reports exhaustion", async () => { const { deps, task } = setup(); await dispatchQueuedPlayback(request, deps); expect(deps.accept).toHaveBeenCalledWith(expect.objectContaining({ trackIds: ["a", "a", "b"], queueEntryIds: ["a-1", "a-2", "b-1"] })); expect(deps.update).not.toHaveBeenCalled(); task.resolve({ status: "exhausted", detail: { checked: 19 } }); await pulse(); expect(deps.update).toHaveBeenCalledWith({ status: "exhausted", detail: { checked: 19 } }); });
  it("recovers lost acceptance ACK without stopping accepted playback", async () => { const { deps, task } = setup(); deps.accept = vi.fn(async () => { throw Error("lost ACK"); }); await dispatchQueuedPlayback(request, deps); expect(deps.lookup).toHaveBeenCalled(); expect(deps.stopOwned).not.toHaveBeenCalled(); task.resolve({ status: "playing", detail: {} }); await pulse(); expect(deps.update).toHaveBeenCalledWith({ status: "playing", detail: {} }); });
  it("retains ambiguous delivery if both ACK and recovery read are lost", async () => { const { deps } = setup(); deps.accept = vi.fn(async () => { throw Error("lost"); }); deps.lookup = vi.fn(async () => { throw Error("offline"); }); await dispatchQueuedPlayback(request, deps); expect(deps.stopOwned).not.toHaveBeenCalled(); });
  it("rejected late acceptance stops only its current owner", async () => { const { deps } = setup(); deps.accept = vi.fn(async () => ({ accepted: false })); await dispatchQueuedPlayback(request, deps); expect(deps.stopOwned).toHaveBeenCalledTimes(1); });
  it("replacement during acceptance cannot be stopped by an old receipt", async () => { const { deps, replace } = setup(); deps.accept = vi.fn(async () => { replace(); return { accepted: false }; }); await dispatchQueuedPlayback(request, deps); expect(deps.stopOwned).not.toHaveBeenCalled(); });
  it("cancel during lookup prevents queue installation/start", async () => { const { deps, replace } = setup(); const load = deferred<{
    id: string;
  }[]>(); deps.load = () => load.promise; const run = dispatchQueuedPlayback(request, deps); replace(); load.resolve([{ id: "a" }, { id: "a" }, { id: "b" }]); await run; expect(deps.prepare).not.toHaveBeenCalled(); expect(deps.accept).not.toHaveBeenCalled(); });
  it("old task completion only reports supersession", async () => { const { deps, task, replace } = setup(); await dispatchQueuedPlayback(request, deps); replace(); task.resolve({ status: "playing", detail: {} }); await pulse(); expect(deps.update).toHaveBeenCalledWith({ status: "superseded", detail: {} }); });
  it("detached rejection is observed and reported", async () => { const { deps, task } = setup(); await dispatchQueuedPlayback(request, deps); task.reject(Error("provider failed")); await pulse(); expect(deps.update).toHaveBeenCalledWith({ status: "failed", detail: { reason: "Error: provider failed" } }); });
  it("rejects wrong occurrence multiplicity before start", () => { expect(() => exactQueuedTracks(["a", "a", "b"], [{ id: "a" }, { id: "b" }, { id: "b" }])).toThrow(); });
});
import { PlaybackQueue } from "./playback-queue";
import { appendTrackFailure, shouldSkipFailedTrack } from "./playback-failure";
import type { Track, PlaybackTrackFailure } from "./types";
it("keeps a 19-occurrence WebView failure run owned beyond 45 seconds without repeat or radio", async () => {
  vi.useFakeTimers();
  try {
    const tracks = Array.from({ length: 19 }, (_, i) => ({
      id: i === 1 ? "track-0" : `track-${i}`, provider: "test", providerId: String(i), title: `Recording ${i}`, artist: "Artist", sourceUrl: "", favorite: false, provenance: {}
    } as Track));
    const q = new PlaybackQueue();
    const accepted = vi.fn(async () => ({ accepted: true }));
    const report = vi.fn(async () => ({}));
    let history: PlaybackTrackFailure[] = [];
    let stableIds: string[] = [];
    const run = {
      operationId: "album-job", requestId: "rpc-album", trackIds: tracks.map(t => t.id)
    };
    await dispatchQueuedPlayback<Track>(run, {
      load: async () => tracks, current: () => true,
      prepare: rows => {
        q.setTracks(rows, 0);
        q.setRepeat("all");
        stableIds = [...q.entryIds];
        const completion = (async (): Promise<QueuedPlayOutcome> => {
          const visited = new Set<string>();
          let generation = 0;
          for (;;) {
            const track = q.current!, entry = q.entryIds[q.currentIndex]!;
            visited.add(entry);
            await new Promise<void>(resolve => setTimeout(resolve, 4000));
            const reason = "No playable audio match was found";
            expect(shouldSkipFailedTrack(reason)).toBe(true);
            const next = q.nextAfterFailedStart(visited);
            history = appendTrackFailure(history, track, {
              queueRevision: "exact-album", generation: ++generation, operationId: run.operationId, entryId: entry, outcome: next ? "skipped" : "queue_exhausted"
            }, reason);
            if (!next)
              return { status: "exhausted", detail: { checked: visited.size } };
          }
        })();
        return { receipt: {
            engine: "webview", trackIds: run.trackIds, queueRevision: "exact-album", queueEntryIds: stableIds, acceptedGeneration: 1
          }, completion };
      }, accept: accepted, lookup: async () => ({ status: "accepted", receipt: {} }), stopOwned: () => { throw Error("owned playback should survive receipt completion"); }, update: report,
    });
    expect(accepted).toHaveBeenCalledTimes(1);
    expect(history).toHaveLength(0);
    await vi.advanceTimersByTimeAsync(48000);
    expect(history).toHaveLength(12);
    expect(report).not.toHaveBeenCalled();
    await vi.advanceTimersByTimeAsync(28000);
    expect(history).toHaveLength(19);
    expect(history.map(f => f.entryId)).toEqual(stableIds);
    expect(new Set(history.map(f => f.operationId))).toEqual(new Set([run.operationId]));
    expect(history.every(f => f.category === "no_playable_match")).toBe(true);
    expect(history[history.length - 1]?.outcome).toBe("queue_exhausted");
    expect(q.currentIndex).toBe(18);
    expect(report).toHaveBeenCalledWith({ status: "exhausted", detail: { checked: 19 } });
    await vi.advanceTimersByTimeAsync(60000);
    expect(history).toHaveLength(19);
  }
  finally {
    vi.useRealTimers();
  }
});
import { createQueuedPlaybackOutbox } from "./queued-playback";
it("retains terminal status after bounded retries and delivers on a later automation pulse", async () => {
  let offline = true;
  const send = vi.fn(async () => { if (offline)
    throw Error("lost update"); return { accepted: true }; });
  const outbox = createQueuedPlaybackOutbox(send, 2);
  expect(outbox.reserve(request)).toBe(true);
  await outbox.publish(request, { status: "exhausted", detail: { checked: 19 } });
  expect(send).toHaveBeenCalledTimes(3);
  expect(outbox.pendingCount()).toBe(1);
  offline = false;
  await outbox.flush();
  expect(outbox.pendingCount()).toBe(0);
  expect(send).toHaveBeenLastCalledWith(request, { status: "exhausted", detail: { checked: 19 } });
});
it("bounds undelivered reports and blocks new dispatch reservations until delivery recovers", async () => {
  let offline = true;
  const outbox = createQueuedPlaybackOutbox(async () => { if (offline)
    throw Error("offline"); return { accepted: true }; }, 1);
  expect(outbox.reserve(request)).toBe(true);
  await outbox.publish(request, { status: "failed", detail: {} });
  expect(outbox.reserve({ ...request, operationId: "next" })).toBe(false);
  expect(outbox.pendingCount()).toBe(1);
  offline = false;
  await outbox.flush();
  expect(outbox.reserve({ ...request, operationId: "next" })).toBe(true);
});
it("an in-flight old status ACK cannot delete a newer pending terminal status", async () => {
  const first = deferred<{
    accepted: boolean;
  }>();
  let calls = 0;
  const outbox = createQueuedPlaybackOutbox(async () => { if (++calls === 1)
    return first.promise; return { accepted: true }; });
  const playing = outbox.publish(request, { status: "playing", detail: {} });
  const cancelled = outbox.publish(request, { status: "cancelled", detail: {} });
  first.resolve({ accepted: true });
  await playing;
  await cancelled;
  expect(outbox.pendingCount()).toBe(1);
  await outbox.flush();
  expect(outbox.pendingCount()).toBe(0);
});
