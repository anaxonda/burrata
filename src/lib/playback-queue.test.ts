import { describe, expect, it } from "vitest";
import { PlaybackQueue, planQueueSelectionMove, queueRemovalIndexes, loadPlaybackQueue, savePlaybackQueue } from "./playback-queue";
import type { Track } from "./types";

const track = (id: string) => ({ id, provider: "test", providerId: id, sourceUrl: "", artist: "A", title: id, favorite: false, provenance: {} }) as Track;

describe("PlaybackQueue", () => {
  it("reorders by current occurrence index even when the identical Track object repeats",()=>{
    const same=track("same"),other=track("other");
    const q=new PlaybackQueue();q.setTracks([same,same,other],1);
    q.reorder(2,0);expect(q.currentIndex).toBe(2);expect(q.current).toBe(same);
    q.reorder(0,2);expect(q.currentIndex).toBe(1);
    q.reorder(1,0);expect(q.currentIndex).toBe(0);
    q.reorder(1,2);expect(q.currentIndex).toBe(0);
    q.reorder(0,2);expect(q.currentIndex).toBe(2);
    expect(q.tracks).toEqual([other,same,same]);
  });

  it("keeps opaque occurrence IDs attached through duplicate reorder, append, and play-next",()=>{
    const same=track("same"),other=track("other");
    const q=new PlaybackQueue();q.setTracks([same,same,other],1);
    const [first,second,third]=q.entryIds;
    expect(new Set(q.entryIds).size).toBe(3);
    expect(q.currentEntry?.entryId).toBe(second);
    q.reorder(0,2);
    expect(q.entryIds).toEqual([second,third,first]);
    const appended=q.appendTracks([same,same]);
    const next=q.appendNextTracks([same,same]);
    expect(appended).toHaveLength(2);expect(next).toHaveLength(2);
    expect(new Set(q.entryIds).size).toBe(q.entryIds.length);
    expect(q.entryIds.slice(1,3)).toEqual(next.map((entry)=>entry.entryId));
    expect(q.tracks.filter((item)=>item===same)).toHaveLength(6);
    expect(q.entries.filter((entry)=>entry.track===same).map((entry)=>entry.entryId)).toEqual([second,...next.map((entry)=>entry.entryId),first,...appended.map((entry)=>entry.entryId)]);
  });

  it("keeps album normalization bound to exact duplicate queue occurrences and survives restore",()=>{
    const sameRecordingA={...track("same"),assetId:"asset-a"} as Track;
    const sameRecordingB={...track("same"),assetId:"asset-b"} as Track;
    const resource={
      scope:"album" as const,
      members:["asset-a","asset-b"].map(assetId=>({assetId,fileHash:"a".repeat(64),range:{startSeconds:0,endSeconds:null}})),
      interTrackGapsSeconds:[0],resourceKey:"b".repeat(64),
    };
    const q=new PlaybackQueue();q.setTracks([sameRecordingA,sameRecordingB,track("other")]);
    const [first,second,other]=q.entryIds;
    expect(()=>q.setLoudnessContext([first!,second!],{...resource,interTrackGapsSeconds:[0.5]})).toThrow();
    q.setLoudnessContext([first!,second!],resource);
    q.reorder(1,0);
    expect(q.loudnessContextFor(first)).toBe(resource);
    expect(q.loudnessContextFor(second)).toBe(resource);
    expect(q.loudnessContextFor(other)).toBeNull();
    const restored=new PlaybackQueue();restored.restore(q.snapshot());
    expect(restored.loudnessContextFor(first)).toEqual(resource);
    expect(restored.loudnessContextFor(second)).toEqual(resource);
    restored.removeAt(restored.entryIds.indexOf(first!));
    expect(restored.loudnessContextFor(first)).toBeNull();
    expect(restored.loudnessContextFor(second)).toEqual(resource);
  });

  it("sorts duplicate-reference occurrences by their own track values and keeps each ID stable",()=>{
    const current=track("current"),same=track("same");
    const zed={...track("zed"),title:"Zed"} as Track;
    const alpha={...track("alpha"),title:"Alpha"} as Track;
    const q=new PlaybackQueue();q.setTracks([current,same,zed,same,alpha],0,["current","same-1","zed-1","same-2","alpha-1"]);
    q.sortUpcoming("title","asc");
    expect(q.entryIds).toEqual(["current","alpha-1","same-1","same-2","zed-1"]);
    expect(q.entries[2]?.track).toBe(same);expect(q.entries[3]?.track).toBe(same);
    expect(q.currentEntry?.entryId).toBe("current");
  });

  it("preserves occurrence IDs for same-length payload refreshes and rejects length changes",()=>{
    const q=new PlaybackQueue();q.setTracks([track("same"),track("same")],1,["first","second"]);
    const changed=[{...track("same"),title:"Refreshed one"},{...track("same"),title:"Refreshed two"}] as Track[];
    expect(q.updateTracks(changed)).toBe(true);
    expect(q.entryIds).toEqual(["first","second"]);
    expect(q.currentEntry?.entryId).toBe("second");
    expect(q.tracks.map((item)=>item.title)).toEqual(["Refreshed one","Refreshed two"]);
    expect(q.updateTracks([track("short")])).toBe(false);
    expect(q.entryIds).toEqual(["first","second"]);
    expect(q.tracks).toEqual(changed);
  });
  it("follows the displayed order and supports play next, removal, and reordering", () => {
    const q = new PlaybackQueue(); q.setTracks([track("a"), track("b"), track("c")], 1);
    q.playNext(track("x")); expect(q.tracks.map((t) => t.id)).toEqual(["a", "b", "x", "c"]);
    expect(q.next()?.id).toBe("x"); q.remove("b"); expect(q.current?.id).toBe("x");
    q.reorder(2, 0); expect(q.current?.id).toBe("x"); expect(q.tracks.map((t) => t.id)).toEqual(["c", "a", "x"]);
  });

  it("implements repeat modes and shuffles only upcoming tracks", () => {
    const q = new PlaybackQueue(); q.setTracks(["a", "b", "c", "d"].map(track), 1);
    q.shuffle(() => 0); expect(q.tracks[0]?.id).toBe("a"); expect(q.tracks[1]?.id).toBe("b");
    q.setRepeat("one"); expect(q.next()?.id).toBe("b");
    q.setRepeat("all"); q.setTracks([track("a"), track("b")], 1); expect(q.next()?.id).toBe("a");
    q.setRepeat("off"); expect(q.next()?.id).toBe("b"); expect(q.next()).toBeNull();
  });

  it("sorts upcoming occurrences while preserving played rows and the current item", () => {
    const played = track("played");
    const current = track("current");
    const zed = { ...track("zed"), title: "Zed" } as Track;
    const duplicateA = { ...track("duplicate"), title: "Same" } as Track;
    const alpha = { ...track("alpha"), title: "Alpha" } as Track;
    const duplicateB = { ...track("duplicate"), title: "Same" } as Track;
    const queue = new PlaybackQueue();
    queue.setTracks([played, current, zed, duplicateA, alpha, duplicateB], 1);

    queue.sortUpcoming("title", "asc");

    expect(queue.tracks.slice(0, 2)).toEqual([played, current]);
    expect(queue.current).toBe(current);
    expect(queue.currentIndex).toBe(1);
    expect(queue.tracks.slice(2)).toEqual([alpha, duplicateA, duplicateB, zed]);
    expect(queue.tracks[3]).toBe(duplicateA);
    expect(queue.tracks[4]).toBe(duplicateB);
  });

  it("persists and restores snapshots safely", () => {
    const values = new Map<string, string>();
    const storage = { getItem: (k: string) => values.get(k) ?? null, setItem: (k: string, v: string) => { values.set(k, v); } } as Storage;
    const q = new PlaybackQueue(); q.setTracks([track("a"), track("b")], 1); q.setRepeat("all");
    expect(savePlaybackQueue(q, storage)).toBe(true);
    const restored = loadPlaybackQueue(storage)!;
    expect(restored.current?.id).toBe("b"); expect(restored.repeat).toBe("all");
    expect(restored.entryIds).toEqual(q.entryIds);
  });

  it("restores the distinct IDs of repeated queue occurrences after reload",()=>{
    const values=new Map<string,string>();
    const storage={getItem:(key:string)=>values.get(key)??null,setItem:(key:string,value:string)=>{values.set(key,value);}} as Storage;
    const same=track("same"),queue=new PlaybackQueue();
    queue.setTracks([same,same],1);queue.setRepeat("all");
    const originalIds=[...queue.entryIds];
    expect(originalIds[0]).not.toBe(originalIds[1]);
    expect(savePlaybackQueue(queue,storage)).toBe(true);
    const reloaded=loadPlaybackQueue(storage)!;
    expect(reloaded.entryIds).toEqual(originalIds);
    expect(reloaded.currentEntry?.entryId).toBe(originalIds[1]);
    expect(reloaded.tracks.map((item)=>item.id)).toEqual(["same","same"]);
    expect(reloaded.repeat).toBe("all");
  });

  it("restores legacy Track arrays and preserves optional chapter ranges", () => {
    const values = new Map<string, string>([["burrata.playback-queue.v1", JSON.stringify({ tracks: [track("legacy")], currentIndex: 0 })]]);
    const storage = { getItem: (k: string) => values.get(k) ?? null, setItem: (k: string, v: string) => { values.set(k, v); } } as Storage;
    const legacy = loadPlaybackQueue(storage)!;
    expect(legacy.current?.id).toBe("legacy");
    expect(legacy.repeat).toBe("off");
    expect(legacy.snapshot().entryIds).toHaveLength(1);
    expect(new Set(legacy.snapshot().entryIds).size).toBe(1);
    expect(savePlaybackQueue(legacy,storage)).toBe(true);
    const upgraded=JSON.parse(values.get("burrata.playback-queue.v1")!) as {entryIds:string[]};
    expect(upgraded.entryIds).toEqual(legacy.entryIds);
    const ranged = { ...track("chapter"), playbackRange: { parentTrackId: "parent", chapterId: "c1", sourceUrl: "https://audio.test", startTime: 10, endTime: 20, parentTitle: "Mix" } } as Track;
    const queue = new PlaybackQueue(); queue.setTracks([ranged]);
    expect(savePlaybackQueue(queue, storage)).toBe(true);
    expect(loadPlaybackQueue(storage)?.current).toMatchObject({ id: "chapter", playbackRange: { startTime: 10, endTime: 20 } });
  });

  it("validates persisted occurrence IDs and mints distinct IDs for malformed legacy data",()=>{
    const duplicate=track("same");
    const q=new PlaybackQueue();q.restore({tracks:[duplicate,duplicate],currentIndex:1,repeat:"off",entryIds:["same-id","same-id"]});
    expect(q.entryIds).toHaveLength(2);
    expect(new Set(q.entryIds).size).toBe(2);
    expect(q.currentEntry?.entryId).toBe(q.entryIds[1]);
    const accepted=new PlaybackQueue();accepted.restore({tracks:[duplicate,duplicate],currentIndex:0,repeat:"off",entryIds:["opaque-1","opaque-2"]});
    expect(accepted.entryIds).toEqual(["opaque-1","opaque-2"]);
  });

  it("removes the chosen duplicate occurrence by index", () => {
    const duplicate = track("same");
    const q = new PlaybackQueue(); q.setTracks([duplicate, track("middle"), duplicate], 2);
    expect(q.removeAt(0)).toBe(duplicate);
    expect(q.currentIndex).toBe(1);
    expect(q.current).toBe(duplicate);
    expect(q.tracks.map((t) => t.id)).toEqual(["middle", "same"]);
    expect(q.removeAt(8)).toBeNull();
  });

  it.each(["off","one","all"] as const)("finite failed-start traversal ignores %s repeat and visits duplicate occurrences once",mode=>{
    const q=new PlaybackQueue();const same=track("same");q.setTracks([same,same,...["c","d","e"].map(track)]);q.setRepeat(mode);
    const visited=new Set<string>();const ids=q.entryIds;
    for(let index=0;index<5;index++){
      expect(q.currentIndex).toBe(index);expect(visited.has(ids[index]!)).toBe(false);visited.add(ids[index]!);
      if(index<4)expect(q.nextAfterFailedStart(visited)).toBe(q.tracks[index+1]);
      else expect(q.nextAfterFailedStart(visited)).toBeNull();
    }
    expect(q.currentIndex).toBe(4);expect(q.repeat).toBe(mode);expect(visited.size).toBe(5);
    expect(q.nextAfterFailedStart(visited)).toBeNull();
  });
  it("refuses an already visited next occurrence without changing index",()=>{
    const q=new PlaybackQueue();q.setTracks(["a","b"].map(track));
    expect(q.nextAfterFailedStart(new Set([q.entryIds[1]!]))).toBeNull();expect(q.currentIndex).toBe(0);
  });
  it("can retain an older durable queue when browser storage rejects the latest save", () => {
    const values = new Map<string, string>();
    let rejectWrites = false;
    const storage = {
      getItem: (key: string) => values.get(key) ?? null,
      setItem: (key: string, value: string) => {
        if (rejectWrites) throw new Error("QuotaExceededError fixture");
        values.set(key, value);
      },
    } as Storage;
    const queue = new PlaybackQueue();
    queue.setTracks(Array.from({ length: 200 }, (_, index) => track(`old-${index}`)), 5);
    expect(savePlaybackQueue(queue, storage)).toBe(true);
    rejectWrites = true;
    queue.setTracks([track("latest")], 0);
    expect(savePlaybackQueue(queue, storage)).toBe(false);
    expect(queue.tracks).toHaveLength(1);
    expect(queue.current?.id).toBe("latest");
    const afterRestart = loadPlaybackQueue(storage)!;
    expect(afterRestart.tracks).toHaveLength(200);
    expect(afterRestart.currentIndex).toBe(5);
    expect(afterRestart.current?.id).toBe("old-5");
  });

});

describe("queue occurrence move planning",()=>{
  it("keeps duplicate occurrences and selected order while moving a block",()=>{
    const plan=planQueueSelectionMove(5,[1,2],2);
    const rows=["same","same","chapter","d","e"];
    for(const [from,to] of plan.moves)rows.splice(to,0,rows.splice(from,1)[0]!);
    expect(rows).toEqual(["same","d","same","chapter","e"]);
    expect(plan.selected).toEqual([2,3]);
    expect(planQueueSelectionMove(3,[0],-1).moves).toEqual([]);
    expect(planQueueSelectionMove(3,[99,NaN],1).moves).toEqual([]);
  });
  it("plans descending removal indexes by occurrence and filters invalid selections",()=>{
    expect(queueRemovalIndexes(5,[1,3,1,99,-1,NaN])).toEqual([3,1]);
    expect(queueRemovalIndexes(0,[0])).toEqual([]);
  });
});
