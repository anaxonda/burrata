import { describe, expect, it, vi } from "vitest";
import { WebviewNextCandidatePrefetch, type UpcomingPlayback } from "./webview-next-candidate-prefetch";
import type { Track } from "./types";
const track = (id: string): Track => ({ id, provider: "spotify", title: id, artist: "Artist" } as Track);
const queue = (overrides: Partial<UpcomingPlayback> = {}): UpcomingPlayback => ({ webview: true, playing: true, generation: 1, session: "playback-1", currentTrackId: "a", queue: { tracks: [track("a"), track("b"), track("c")], entryIds: ["occ-a", "occ-b", "occ-c"], currentIndex: 0, repeat: "off" }, ...overrides });
describe("WebView next candidate search", () => {
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
  it("does not restart cancelled work on the same playing/position occurrence", () => {
    const invoke=vi.fn((_command:string,_args:Record<string,unknown>)=>new Promise(()=>{}));const prefetch=new WebviewNextCandidatePrefetch(invoke,()=>"own");
    prefetch.started(queue());prefetch.cancel();for(let i=0;i<100;i++)prefetch.started(queue());
    expect(invoke.mock.calls.filter(([name])=>name==="warm_playback_candidate_search")).toHaveLength(1);
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
