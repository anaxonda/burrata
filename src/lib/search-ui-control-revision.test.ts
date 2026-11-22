import { describe, expect, it } from "vitest";
import { captureSearchUiControlSnapshot, extendSearchUiControlSnapshot, searchUiControlExpired, type SearchUiOccurrenceIdentity } from "./search-ui-control-revision";

const track = { id:"chapter:parent:one", provider:"youtube", providerId:"source", sourceUrl:"https://example.test/source", title:"Mix", artist:"Artist", version:"", durationSeconds:30,
  playbackRange:{parentTrackId:"parent",chapterId:"one",sourceUrl:"https://example.test/source",startTime:10,endTime:40,parentTitle:"Mix",parentDurationSeconds:100} };
const input = (value: SearchUiOccurrenceIdentity["track"]) => ({text:"mix",submitted:"mix",mode:"online",source:"youtube",kind:"tracks",sort:"sourceOrder",direction:"asc",minimumDuration:"",maximumDuration:"",generation:2,collectionRequest:3,continueSearch:true,visibleSources:["youtube"],occurrences:[{sourceId:"youtube",rowId:"chapter-one",rowKind:"track",track:value}]});
describe("search UI control snapshot identity",()=>{
  it("invalidates the revision identity when playback version or source range changes",()=>{
    const base = captureSearchUiControlSnapshot(input(track));
    for (const changed of [
      { ...track, version: "Live Version" },
      { ...track, isrc: "USABC0123456" },
      { ...track, playbackRange: { ...track.playbackRange, startTime: 11 } },
      { ...track, playbackRange: { ...track.playbackRange, endTime: 39 } },
    ]) {
      expect(extendSearchUiControlSnapshot(base, captureSearchUiControlSnapshot(input(changed)))).toBeNull();
    }
  });

  it("accepts appended rows and rejects reorder, removal, and query changes", () => {
    const initial = input(track);
    const [existing] = initial.occurrences;
    const appended = { ...existing, rowId: "chapter-two" };
    const before = captureSearchUiControlSnapshot(initial);
    const after = captureSearchUiControlSnapshot({ ...initial, occurrences: [existing, appended] });
    expect(extendSearchUiControlSnapshot(before, after)).not.toBeNull();
    expect(extendSearchUiControlSnapshot(after, before)).toBeNull();
    expect(extendSearchUiControlSnapshot(before, captureSearchUiControlSnapshot({
      ...initial, occurrences: [appended, existing],
    }))).toBeNull();
    expect(extendSearchUiControlSnapshot(before, captureSearchUiControlSnapshot({
      ...initial, text: "another query",
    }))).toBeNull();
  });

  const video = {
    ...track, id: "youtube:DQCXk4rQmCQ", providerId: "DQCXk4rQmCQ",
    sourceUrl: "https://www.youtube.com/watch?v=DQCXk4rQmCQ", playbackRange: undefined, durationSeconds: 375,
  };
  const snapshot = (value: SearchUiOccurrenceIdentity["track"]) => captureSearchUiControlSnapshot(input(value));
  it("allows a one-second refinement for the same exact YouTube video without accumulating drift", () => {
    const before = snapshot(video);
    const after = extendSearchUiControlSnapshot(before, snapshot({ ...video, durationSeconds: 374 }));
    expect(after).not.toBeNull();
    expect(extendSearchUiControlSnapshot(after!, before)).not.toBeNull();
    expect(extendSearchUiControlSnapshot(after!, snapshot({ ...video, durationSeconds: 373 }))).toBeNull();
    expect(extendSearchUiControlSnapshot(after!, snapshot({ ...video, durationSeconds: 376 }))).toBeNull();
  });

  it("rejects missing or invalid durations, changed identity, and approximate or ranged locators", () => {
    const before = snapshot(video);
    for (const changed of [
      { ...video, durationSeconds: 0 },
      { ...video, durationSeconds: null },
      { ...video, durationSeconds: 374, title: "Another recording" },
      { ...video, durationSeconds: 374, sourceUrl: "https://www.youtube.com/watch?v=another-one" },
    ]) {
      expect(extendSearchUiControlSnapshot(before, snapshot(changed))).toBeNull();
    }
    for (const original of [
      { ...video, sourceUrl: "https://www.youtube.com/results?search_query=one" },
      { ...video, playbackRange: track.playbackRange },
      { ...video, provider: "import" },
    ]) {
      expect(extendSearchUiControlSnapshot(snapshot(original), snapshot({ ...original, durationSeconds: 374 }))).toBeNull();
    }
  });
});

describe("search UI control deadlines",()=>{
  it("rejects invalid and elapsed wall-clock deadlines before acting",()=>{
    expect(searchUiControlExpired(5000,4999)).toBe(false);
    expect(searchUiControlExpired(5000,5000)).toBe(true);
    expect(searchUiControlExpired(Number.NaN,1)).toBe(true);
  });
});
