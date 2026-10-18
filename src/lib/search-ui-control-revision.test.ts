import { describe, expect, it } from "vitest";
import { searchUiControlFingerprint, searchUiControlExpired, type SearchUiOccurrenceIdentity } from "./search-ui-control-revision";

const track = { id:"chapter:parent:one", provider:"youtube", providerId:"source", sourceUrl:"https://example.test/source", title:"Mix", artist:"Artist", version:"", durationSeconds:30,
  playbackRange:{parentTrackId:"parent",chapterId:"one",sourceUrl:"https://example.test/source",startTime:10,endTime:40,parentTitle:"Mix",parentDurationSeconds:100} };
const input = (value: SearchUiOccurrenceIdentity["track"]) => ({text:"mix",submitted:"mix",mode:"online",source:"youtube",kind:"tracks",sort:"sourceOrder",direction:"asc",minimumDuration:"",maximumDuration:"",generation:2,collectionRequest:3,continueSearch:true,occurrences:[{sourceId:"youtube",rowId:"chapter-one",rowKind:"track",track:value}]});
describe("search UI control snapshot identity",()=>{
  it("invalidates the revision identity when playback version or source range changes",()=>{
    const base=searchUiControlFingerprint(input(track));
    expect(searchUiControlFingerprint(input({...track,version:"Live Version"}))).not.toBe(base);
    expect(searchUiControlFingerprint(input({...track,playbackRange:{...track.playbackRange,startTime:11}}))).not.toBe(base);
    expect(searchUiControlFingerprint(input({...track,playbackRange:{...track.playbackRange,endTime:39}}))).not.toBe(base);
  });
});

describe("search UI control deadlines",()=>{
  it("rejects invalid and elapsed wall-clock deadlines before acting",()=>{
    expect(searchUiControlExpired(5000,4999)).toBe(false);
    expect(searchUiControlExpired(5000,5000)).toBe(true);
    expect(searchUiControlExpired(Number.NaN,1)).toBe(true);
  });
});
