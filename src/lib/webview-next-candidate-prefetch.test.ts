import { afterEach, describe, expect, it, vi } from "vitest";
import { WebviewNextCandidatePrefetch, type UpcomingPlayback } from "./webview-next-candidate-prefetch";
import type { Track } from "./types";
const track = (id: string): Track => ({ id, provider: "spotify", title: id, artist: "Artist" } as Track);
const queue = (overrides: Partial<UpcomingPlayback> = {}): UpcomingPlayback => ({ webview: true, playing: true, generation: 1, session: "playback-1", currentTrackId: "a", queue: { tracks: [track("a"), track("b"), track("c")], entryIds: ["occ-a", "occ-b", "occ-c"], currentIndex: 0, repeat: "off" }, ...overrides });
describe("WebView next candidate search", () => {
  afterEach(() => vi.useRealTimers());
  it("searches only the actual next shuffled occurrence once per successful start", () => {
    const invoke = vi.fn((_command:string,_args:Record<string,unknown>) => new Promise(() => {}));
    const prefetch = new WebviewNextCandidatePrefetch(invoke, () => "request-1");
    const input=queue();input.queue.tracks=[track("a"),track("c"),track("b")];
    prefetch.started(input);for(let i=0;i<50;i++)prefetch.started(input);
    expect(invoke).toHaveBeenCalledTimes(1);
    expect(invoke).toHaveBeenCalledWith("warm_playback_candidate_search",{track:input.queue.tracks[1],requestId:"request-1"});
  });
  it("cancels only its own speculative UUID and ignores stale completion", async () => {
    let finishOld!: (value: unknown) => void;let sequence=0;
    const invoke=vi.fn((command:string,_args:Record<string,unknown>)=>command==="warm_playback_candidate_search" && sequence===1?new Promise(resolve=>{finishOld=resolve;}):new Promise(()=>{}));
    const prefetch=new WebviewNextCandidatePrefetch(invoke,()=>`request-${++sequence}`);
    prefetch.started(queue());prefetch.cancel();
    prefetch.started(queue({generation:2,session:"playback-2"}));
    finishOld(undefined);await Promise.resolve();await Promise.resolve();await Promise.resolve();prefetch.cancel();
    expect(invoke.mock.calls.filter(([name])=>name==="cancel_remote_read")).toEqual([["cancel_remote_read",{requestId:"request-1"}],["cancel_remote_read",{requestId:"request-2"}]]);
    expect(invoke.mock.calls.some(([name])=>name==="cancel_playback_resolution")).toBe(false);
  });
  it("retries a cancelled occurrence once without repeated playing-event thrash", () => {
    const invoke=vi.fn((_command:string,_args:Record<string,unknown>)=>new Promise(()=>{}));const prefetch=new WebviewNextCandidatePrefetch(invoke,()=>"own");
    prefetch.started(queue());prefetch.cancel();for(let i=0;i<100;i++)prefetch.started(queue());
    expect(invoke.mock.calls.filter(([name])=>name==="warm_playback_candidate_search")).toHaveLength(2);
    prefetch.cancel(); prefetch.started(queue());
    expect(invoke.mock.calls.filter(([name])=>name==="warm_playback_candidate_search")).toHaveLength(2);
  });
  it("backs off one failed search, retries once, and stops after the second failure", async () => {
    vi.useFakeTimers();
    let sequence = 0;
    const invoke = vi.fn(async (_command: string, _args: Record<string, unknown>) => { throw new Error("search unavailable"); });
    const prefetch = new WebviewNextCandidatePrefetch(invoke, () => `retry-${++sequence}`);
    prefetch.started(queue()); await Promise.resolve();
    for (let i = 0; i < 100; i++) prefetch.started(queue());
    expect(invoke).toHaveBeenCalledTimes(1);
    await vi.advanceTimersByTimeAsync(999);
    expect(invoke).toHaveBeenCalledTimes(1);
    await vi.advanceTimersByTimeAsync(1);
    expect(invoke).toHaveBeenCalledTimes(2);
    await vi.advanceTimersByTimeAsync(5000);
    for (let i = 0; i < 100; i++) prefetch.started(queue());
    expect(invoke).toHaveBeenCalledTimes(2);
  });
  it("cancels a pending retry when paused and lets a later playing event retry", async () => {
    vi.useFakeTimers();
    let sequence = 0;
    const invoke = vi.fn((command: string, _args: Record<string, unknown>) => command === "warm_playback_candidate_search" ? Promise.reject(new Error("network")) : Promise.resolve());
    const prefetch = new WebviewNextCandidatePrefetch(invoke, () => `pause-${++sequence}`);
    prefetch.started(queue()); await Promise.resolve();
    prefetch.cancel();
    await vi.advanceTimersByTimeAsync(5000);
    expect(invoke.mock.calls.filter(([name])=>name==="warm_playback_candidate_search")).toHaveLength(1);
    prefetch.started(queue()); await Promise.resolve();
    expect(invoke.mock.calls.filter(([name])=>name==="warm_playback_candidate_search")).toHaveLength(2);
    await vi.advanceTimersByTimeAsync(5000);
    expect(invoke.mock.calls.filter(([name])=>name==="warm_playback_candidate_search")).toHaveLength(2);
  });
  it("reordering an uninterrupted playing queue warms the new next occurrence and fences the old completion", async () => {
    let finishOld!: () => void;
    let sequence = 0;
    const invoke = vi.fn((command: string, _args: Record<string, unknown>) => command === "warm_playback_candidate_search" && sequence === 1 ? new Promise<void>(resolve => { finishOld = resolve; }) : new Promise<void>(() => {}));
    const prefetch = new WebviewNextCandidatePrefetch(invoke, () => `reorder-${++sequence}`);
    prefetch.started(queue());
    const changed = queue({generation: 2});
    changed.queue.tracks = [track("a"), track("c"), track("b")];
    changed.queue.entryIds = ["occ-a", "occ-c", "occ-b"];
    prefetch.started(changed);
    expect(invoke).toHaveBeenCalledWith("cancel_remote_read", {requestId: "reorder-1"});
    expect(invoke).toHaveBeenLastCalledWith("warm_playback_candidate_search", {track: track("c"), requestId: "reorder-2"});
    finishOld(); await Promise.resolve();
    prefetch.started(changed);
    expect(invoke.mock.calls.filter(([name])=>name==="warm_playback_candidate_search")).toHaveLength(2);
    prefetch.cancel();
    expect(invoke).toHaveBeenLastCalledWith("cancel_remote_read", {requestId: "reorder-2"});
  });
  it("keeps early search distinct and starts validated source preparation 45 seconds before handoff", async () => {
    vi.useFakeTimers();
    let sequence = 0;
    const invoke = vi.fn(async (_command: string, _args: Record<string, unknown>) => undefined);
    const prefetch = new WebviewNextCandidatePrefetch(invoke, () => `lead-${++sequence}`);
    prefetch.started(queue({remainingSeconds: 120}));
    expect(invoke).toHaveBeenCalledWith("warm_playback_candidate_search", {track: track("b"), requestId: "lead-1"});
    expect(invoke.mock.calls.some(([name]) => name === "prepare_upcoming_playback_source")).toBe(false);
    await vi.advanceTimersByTimeAsync(74_999);
    expect(invoke.mock.calls.some(([name]) => name === "prepare_upcoming_playback_source")).toBe(false);
    await vi.advanceTimersByTimeAsync(1);
    expect(invoke).toHaveBeenLastCalledWith("prepare_upcoming_playback_source", {
      track: track("b"), requestId: "lead-2", handoffInMs: 45_000,
    });
    prefetch.cancel();
  });
  it("uses known short-track time and cancels scheduled work on queue change", async () => {
    vi.useFakeTimers();
    let sequence = 0;
    const invoke = vi.fn(async (_command: string, _args: Record<string, unknown>) => undefined);
    const prefetch = new WebviewNextCandidatePrefetch(invoke, () => `short-${++sequence}`);
    prefetch.started(queue({remainingSeconds: 30}));
    await vi.advanceTimersByTimeAsync(0);
    expect(invoke).toHaveBeenLastCalledWith("prepare_upcoming_playback_source", {
      track: track("b"), requestId: "short-2", handoffInMs: 30_000,
    });
    const changed = queue({generation: 2, remainingSeconds: 120});
    changed.queue.repeat = "one";
    prefetch.started(changed);
    await vi.advanceTimersByTimeAsync(120_000);
    expect(invoke.mock.calls.filter(([name]) => name === "prepare_upcoming_playback_source")).toHaveLength(1);
    prefetch.cancel();
  });
  it("respects native mode, repeat-one, radio, local/chapter, stale current and queue end", () => {
    for(const input of [queue({webview:false}),queue({playing:false}),queue({currentTrackId:"stale"}),queue({pinnedTrackIds:["b"]})]){
      const invoke=vi.fn(async(_command:string,_args:Record<string,unknown>)=>{});new WebviewNextCandidatePrefetch(invoke).started(input);expect(invoke).not.toHaveBeenCalled();
    }
    const mutations: ((value: UpcomingPlayback["queue"]) => void)[] = [q=>{q.repeat="one";},q=>{q.currentIndex=2;},q=>{q.tracks[1]={...track("b"),provider:"radio"};},q=>{q.tracks[1]={...track("b"),provider:"youtube"};},q=>{q.tracks[1]={...track("b"),provider:"soundcloud"};},q=>{q.tracks[1]={...track("b"),localPath:"/music/local.mp3"};}];
    for(const mutate of mutations){
      const input=queue();mutate(input.queue);if(input.queue.currentIndex===2)input.currentTrackId="c";const invoke=vi.fn(async(_command:string,_args:Record<string,unknown>)=>{});new WebviewNextCandidatePrefetch(invoke).started(input);expect(invoke).not.toHaveBeenCalled();
    }
    const input=queue();input.queue.currentIndex=2;input.queue.repeat="all";input.currentTrackId="c";const invoke=vi.fn((_command:string,_args:Record<string,unknown>)=>new Promise(()=>{}));new WebviewNextCandidatePrefetch(invoke,()=>"wrap").started(input);expect(invoke).toHaveBeenCalledWith("warm_playback_candidate_search",{track:input.queue.tracks[0],requestId:"wrap"});
  });
});
