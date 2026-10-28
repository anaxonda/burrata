import { expect, it } from "vitest";
import { sortLyricCandidates, lyricRuntime, lyricDurationDifference } from "./lyric-candidates";
it("sorts by distance from the current runtime without mutating provider match order",()=>{
 const rows=[{id:"a",durationSeconds:151,automaticMatch:false},{id:"b",durationSeconds:156,automaticMatch:false},{id:"c",durationSeconds:161,automaticMatch:false}];
 expect(sortLyricCandidates(rows,156.933,"duration").map(x=>x.id)).toEqual(["b","c","a"]);
 expect(sortLyricCandidates(rows,156.933,"match").map(x=>x.id)).toEqual(["a","b","c"]);
 expect(sortLyricCandidates(rows,156.933,"longest").map(x=>x.id)).toEqual(["c","b","a"]);
 expect(rows.map(x=>x.id)).toEqual(["a","b","c"]);
 expect(lyricRuntime(156.933)).toBe("2:37");
 expect(lyricDurationDifference(151,156.933)).toBe("0:06 shorter");
 expect(lyricDurationDifference(156.933,156.933)).toBe("Same duration");
 expect(lyricDurationDifference(151,null)).toBe("");
});
