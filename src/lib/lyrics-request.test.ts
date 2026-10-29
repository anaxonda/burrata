import { describe, expect, it } from "vitest";
import { visibleLyricsTrack, lyricsTrackWithPlaybackDuration, lyricsPlaybackPosition } from "./lyrics-request";

describe("visibleLyricsTrack", () => {
  it("passes the reported collaboration title, credits, album, and precise duration", () => {
    expect(visibleLyricsTrack({
      id: "reported-nunca", provider: "youtube", providerId: "reported", sourceUrl: "https://youtube.test/watch?v=reported",
      title: "Nunca vas a comprender", artist: "Rita Payés & Elisabeth Roma", album: "Como la piel", durationSeconds: 244.631,
      favorite: false, provenance: {},
    })).toEqual({
      trackId: "reported-nunca", localPath: null, trackName: "Nunca vas a comprender", artistName: "Rita Payés & Elisabeth Roma", albumName: "Como la piel",
      durationSeconds: 244.631, playbackRange: null,
    });
  });

  it("keeps missing album optional and passes excerpts for backend rejection", () => {
    expect(visibleLyricsTrack({
      id: "excerpt", provider: "youtube", providerId: "excerpt", sourceUrl: "https://youtube.test/watch?v=excerpt",
      title: "Synthetic excerpt", artist: "Synthetic Artist", durationSeconds: 50, playbackRange: { parentTrackId: "parent", chapterId: "chapter", sourceUrl: "source", startTime: 1, endTime: 20, parentTitle: "Synthetic source" },
      favorite: false, provenance: {},
    })).toMatchObject({ albumName: null, durationSeconds: 50, playbackRange: { parentTrackId: "parent" } });
    expect(visibleLyricsTrack(null)).toBeUndefined();
  });
});

it("uses only the current full track's decoded runtime and preserves saved metadata", () => {
  const track = {id:"import:song",provider:"import",providerId:"song",sourceUrl:"https://example.test/song",title:"The Way It Goes",artist:"Gillian Welch",durationSeconds:null,favorite:false,provenance:{}};
  const snapshot = {engine:"native" as const, trackId:track.id, duration:241.92};
  expect(lyricsTrackWithPlaybackDuration(track,snapshot)?.durationSeconds).toBe(241.92);
  expect(track.durationSeconds).toBeNull();
  expect(lyricsTrackWithPlaybackDuration(track,{...snapshot,trackId:"other"})).toBe(track);
  expect(lyricsTrackWithPlaybackDuration(track,{...snapshot,duration:0})).toBe(track);
  const excerpt = {...track,provenance:{approximateExcerpt:{startSeconds:10,endSeconds:30}}};
  expect(lyricsTrackWithPlaybackDuration(excerpt,snapshot)).toBe(excerpt);
  const known = {...track,durationSeconds:242};
  expect(lyricsTrackWithPlaybackDuration(known,snapshot)).toBe(known);
});

it("marks identified live radio as untimed without using stream duration",()=>{
 const track={id:"radio:station",provider:"radio",providerId:"station",sourceUrl:"https://example.test/stream",title:"Identified song",artist:"Identified performer",durationSeconds:9000,favorite:false,provenance:{radioMetadata:{title:"Identified song",artist:"Identified performer"}}};
 expect(visibleLyricsTrack(track)).toMatchObject({trackName:"Identified song",artistName:"Identified performer",durationSeconds:0,radio:true});
 expect(track.durationSeconds).toBe(9000);
});

it("uses chapter-local time for lyrics while keeping full-track time for ordinary tracks",()=>{
 const chapter={id:"set:chapter",provider:"youtube",providerId:"set",sourceUrl:"https://example.test/set",title:"Detected song",artist:"Detected artist",durationSeconds:180,favorite:false,provenance:{},playbackRange:{parentTrackId:"set",chapterId:"chapter",sourceUrl:"https://example.test/set",startTime:600,endTime:780,parentTitle:"Long set"}};
 expect(lyricsPlaybackPosition(chapter,615)).toBe(15);
 expect(lyricsPlaybackPosition(chapter,590)).toBe(0);
 expect(lyricsPlaybackPosition({...chapter,playbackRange:undefined},615)).toBe(615);
});

it("projects resource-scoped detections as untimed lyrics identities",()=>{
 const parent={id:"set",provider:"youtube",providerId:"set-id",sourceUrl:"https://example.test/set",title:"Long set",artist:"DJ",durationSeconds:3600,favorite:false,provenance:{}};
 const detection={trackId:"set",generation:4,seekSerial:2,resourceKey:"resource-key",detectedAtSeconds:615};
 expect(visibleLyricsTrack({...parent,title:"Detected song",artist:"Detected artist",id:"set:detected:615"},detection)).toMatchObject({trackId:"set",trackName:"Detected song",artistName:"Detected artist",durationSeconds:0,playbackRange:null,detection});
});
