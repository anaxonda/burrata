import { expect, it } from "vitest";
import type { Track } from "./types";
import { autoplayTrackWithLibraryMetadata, createAutoplayMetadataLookup } from "./autoplay-track-metadata";

const queued: Track = {id: "youtube:u_TSn1iO-hg", provider: "youtube", providerId: "u_TSn1iO-hg",
  sourceUrl: "https://www.youtube.com/watch?v=u_TSn1iO-hg", artist: "", title: "Qué Es Lo Que Pasa",
  favorite: false, durationSeconds: 233, provenance: {sourceKind: "youtubeRadio", audioCandidate: {id: "youtube:u_TSn1iO-hg"}}};
const saved: Track = {...queued, artist: "Macha Y El Bloque Depresivo, Vicente Cifuentes", album: "Qué Es Lo Que Pasa",
  sourceUrl: "https://music.youtube.com/watch?v=u_TSn1iO-hg", provenance: {}};

it("uses the same saved video's credits for display and lyrics while preserving autoplay binding", () => {
  const result = autoplayTrackWithLibraryMetadata(queued, [saved]);
  expect(result.artist).toBe(saved.artist);
  expect(result.album).toBe(saved.album);
  expect(result.sourceUrl).toBe(queued.sourceUrl);
  expect(result.provenance).toBe(queued.provenance);
  expect(queued.artist).toBe("");
});

it("rejects another upload, conflicting source IDs, ambiguous saved entries, and excerpts", () => {
  for (const library of [[], [{...saved, providerId: "otherVideo1"}], [{...saved, sourceUrl: "https://youtube.com/watch?v=otherVideo1"}], [saved, saved], [{...saved, playbackRange: {parentTrackId:"parent",chapterId:"part",sourceUrl:saved.sourceUrl,startTime:0,endTime:30,parentTitle:"Set"}}]]) {
    expect(autoplayTrackWithLibraryMetadata(queued, library)).toBe(queued);
  }
});

it("preserves known queue metadata and does not project onto catalog requests", () => {
  for (const track of [{...queued, artist: "Known performer"}, {...queued, provenance: {sourceKind: "catalog"}}]) {
    expect(autoplayTrackWithLibraryMetadata(track, [saved])).toBe(track);
  }
});

it("preserves manual blanks and rejects a noncanonical video identity", () => {
  const manualArtist={...queued,provenance:{sourceKind:"youtubeRadio",manualFields:{artist:true}}};
  expect(autoplayTrackWithLibraryMetadata(manualArtist,[saved])).toBe(manualArtist);
  const manualAlbum={...queued,album:null,provenance:{sourceKind:"youtubeRadio",manualFields:{album:true}}};
  expect(autoplayTrackWithLibraryMetadata(manualAlbum,[saved])).toMatchObject({artist:saved.artist,album:null});
  const invalid={...queued,id:"another-provider:arbitrary"};
  expect(autoplayTrackWithLibraryMetadata(invalid,[{...saved,id:invalid.id}])).toBe(invalid);
});

it("reads the exact saved ID outside the loaded page and shares concurrent reads", async () => {
  let calls = 0;
  const lookup = createAutoplayMetadataLookup(async id => { calls++; expect(id).toBe(queued.id); return [saved]; });
  const results = await Promise.all([lookup.load(queued),lookup.load(queued)]);
  expect(calls).toBe(1);
  expect(results.map(track => track.artist)).toEqual([saved.artist,saved.artist]);
  expect(lookup.peek(queued).artist).toBe(saved.artist);
  expect(lookup.peek(queued).provenance).toBe(queued.provenance);
  expect(await lookup.load({...queued,artist:"Known"})).toMatchObject({artist:"Known"});
  expect(calls).toBe(1);
});

it("retries missing saved metadata after a bounded negative cache interval", async () => {
  let time = 0, calls = 0;
  const lookup = createAutoplayMetadataLookup(async () => ++calls === 1 ? [] : [saved], () => time);
  expect(await lookup.load(queued)).toBe(queued);
  time = 14999;
  expect(await lookup.load(queued)).toBe(queued);
  expect(calls).toBe(1);
  time = 15000;
  expect((await lookup.load(queued)).artist).toBe(saved.artist);
  expect(calls).toBe(2);
});
