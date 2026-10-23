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
