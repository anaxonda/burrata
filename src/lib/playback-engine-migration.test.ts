import {expect, it} from "vitest";
import type {NativePlaybackSnapshot, Track} from "./types";
import {queueForEngineMigration} from "./playback-engine-migration";
function snapshot() {
  const track = {id:"same",title:"Duplicate recording"} as Track;
  return {queue:[track,track],queueTotal:2,queueEntryIds:["first","second"],currentIndex:1,repeat:"all",position:22} as NativePlaybackSnapshot & {queueTotal:number;queueEntryIds:string[]};
}
it("preserves duplicate recording occurrences and the selected occurrence",()=>{
  const source=snapshot(), result=queueForEngineMigration(source);
  expect(result).toEqual({tracks:source.queue,entryIds:["first","second"],currentIndex:1,repeat:"all"});
  expect(result.tracks).not.toBe(source.queue);expect(result.entryIds).not.toBe(source.queueEntryIds);
});
it.each([
  {queueTotal:200}, {queueEntryIds:undefined}, {queueEntryIds:["first","first"]},
  {queueEntryIds:["first"]}, {currentIndex:2}, {currentIndex:-1},
  {position:NaN}, {position:-1}, {repeat:"invalid"},
])("refuses incomplete or invalid migration input %j",patch=>{
  expect(()=>queueForEngineMigration({...snapshot(),...patch} as ReturnType<typeof snapshot>)).toThrow("complete playback queue");
});
it("accepts an explicitly empty queue",()=>{
  expect(queueForEngineMigration({...snapshot(),queue:[],queueTotal:0,queueEntryIds:[],currentIndex:-1})).toEqual({tracks:[],entryIds:[],currentIndex:-1,repeat:"all"});
});

import { migrateNativePlayback } from "./playback-engine-migration";
import { vi } from "vitest";
const memory = (values: Record<string, string>) => ({ getItem: (key: string) => values[key] ?? null });
it("keeps an existing Native owner even when competing browser storage is unreadable", async () => {
  const native = snapshot();
  const invoke = vi.fn().mockResolvedValue(native);
  expect(await migrateNativePlayback(invoke, {getItem: () => {throw new Error("unavailable");}}, {volume:.4,shuffle:true})).toBe(native);
  expect(invoke).toHaveBeenCalledTimes(1);
});
it("commits duplicate occurrences and their bookmark before accepting migration", async () => {
  const queue = {tracks:snapshot().queue,entryIds:["first","second"],currentIndex:1,repeat:"all"};
  const storage = memory({"burrata.playback-queue.v1":JSON.stringify(queue),"burrata.playback-position.v1":JSON.stringify({trackId:"same",entryId:"second",position:24,savedAt:1})});
  const invoke = vi.fn().mockResolvedValueOnce({migrationRequired:true}).mockResolvedValueOnce(snapshot());
  await migrateNativePlayback(invoke,storage,{volume:.4,shuffle:true});
  expect(invoke.mock.calls[1][1].legacy).toMatchObject({tracks:queue.tracks,entryIds:queue.entryIds,currentEntryId:"second",index:1,position:24,volume:.4,shuffle:true,repeat:"all"});
});
it("migrates an empty active browser session instead of resurrecting a stale queue", async () => {
  const invoke = vi.fn().mockResolvedValueOnce({migrationRequired:true}).mockResolvedValueOnce(snapshot());
  await migrateNativePlayback(invoke,memory({}),{volume:1,shuffle:false});
  expect(invoke.mock.calls[1][1].legacy).toMatchObject({tracks:[],entryIds:[],currentEntryId:null,position:0});
});
it("leaves legacy state available for retry after a rejected commit", async () => {
  const values = {"burrata.playback-queue.v1":JSON.stringify({tracks:snapshot().queue,currentIndex:1,repeat:"off"})};
  const invoke = vi.fn().mockResolvedValueOnce({migrationRequired:true}).mockRejectedValueOnce(new Error("disk full"));
  await expect(migrateNativePlayback(invoke,memory(values),{volume:1,shuffle:false})).rejects.toThrow("disk full");
  expect(values["burrata.playback-queue.v1"]).toContain('"currentIndex":1');
});
it("refuses to commit a corrupt legacy queue", async () => {
  const invoke = vi.fn().mockResolvedValue({migrationRequired:true});
  await expect(migrateNativePlayback(invoke,memory({"burrata.playback-queue.v1":"broken"}),{volume:1,shuffle:false})).rejects.toThrow("invalid");
  expect(invoke).toHaveBeenCalledTimes(1);
});
