import { describe, expect, it } from "vitest";
import { hasExplicitSourceLock, isCatalogSelection } from "./source-lock";
import type { Candidate, Track } from "./types";

const candidate = { id: "monochrome:12345678901234567", provider: "monochrome", url: "https://tracks.monochrome.st/track/12345678901234567" } as Candidate;
const track: Track = { id: "monochrome:12345678901234567", provider: "monochrome", providerId: "12345678901234567", sourceUrl: "https://tracks.monochrome.st/track/12345678901234567", artist: "Artist", title: "Song", favorite: false, provenance: {} };

describe("source lock classification", () => {
  it.each(["soundcloud", "youtube"])("separates a registered %s file preference from an explicit online candidate pin", provider => {
    const downloaded = { ...track, provider, localPath: "/registered/corrupt.m4a", assetId: "registered-asset", provenance: {sourceKind:"audioSearch",audioCandidate:candidate} };
    expect(hasExplicitSourceLock(downloaded)).toBe(true);
    expect(hasExplicitSourceLock(downloaded,{localPathPins:false})).toBe(false);
    expect(hasExplicitSourceLock(downloaded,{localPathPins:false,selectedCandidate:candidate})).toBe(true);
    for(const provenance of [{sourceKind:"audioSearch",userSelectedSource:true},{explicitSourceLock:true},{playbackMode:"source"},{sourceKind:"youtubeRadio"},{sourceKind:"upload"},{sourceKind:"uploaded"},{sourceKind:"localUpload"}])
      expect(hasExplicitSourceLock({...downloaded,provenance},{localPathPins:false})).toBe(true);
    for(const provider of ["radio","local","upload","slsk","slskd"])
      expect(hasExplicitSourceLock({...downloaded,provider},{localPathPins:false})).toBe(true);
    expect(hasExplicitSourceLock({...downloaded,playbackRange:{parentTrackId:track.id,chapterId:"c",sourceUrl:track.sourceUrl,startTime:0,endTime:20,parentTitle:"Set"}},{localPathPins:false})).toBe(true);
  });

  it("distinguishes explicit selections from cached catalog links", () => {
    expect(isCatalogSelection(track)).toBe(true);
    expect(hasExplicitSourceLock(track, { selectedCandidate: candidate })).toBe(true);
    expect(hasExplicitSourceLock({ ...track, provenance: { sourceKind: "audioSearch" } }, { selectedCandidate: candidate, audioSearchResult: true })).toBe(true);
    expect(hasExplicitSourceLock({ ...track, provenance: { sourceKind: "audioSearch", audioCandidate: candidate } }, { audioSearchResult: true })).toBe(false);
    expect(hasExplicitSourceLock({ ...track, provider: "catalog", provenance: { catalogProvider: "spotify" } }, { selectedCandidate: candidate })).toBe(true);
  });

  it("allows implicit audio links to recover while locking files and ranges", () => {
    expect(hasExplicitSourceLock({ ...track, provider: "import", provenance: { sourceKind: "audioSearch" } })).toBe(false);
    expect(hasExplicitSourceLock({ ...track, provider: "youtube", provenance: { sourceKind: "audioSearch" } })).toBe(false);
    expect(hasExplicitSourceLock({ ...track, localPath: "/music/upload.flac" })).toBe(true);
    expect(hasExplicitSourceLock({ ...track, provider: "local" })).toBe(true);
    expect(hasExplicitSourceLock({ ...track, playbackRange: { parentTrackId: track.id, chapterId: "c", sourceUrl: track.sourceUrl, startTime: 10, endTime: 20, parentTitle: "Set" } })).toBe(true);
  });

  it("keeps explicit lock markers authoritative even on catalog identities", () => {
    expect(hasExplicitSourceLock({ ...track, provenance: { sourceKind: "audioSearch", userSelectedSource: true } })).toBe(true);
    expect(hasExplicitSourceLock({ ...track, provenance: { explicitSourceLock: true } })).toBe(true);
  });

  it("treats captured YouTube source defaults as implicit but preserves manual choices", () => {
    const captured = {...track, provider:"youtube", provenance:{sourceKind:"playlist",browserImport:true,captureProvider:"youtube",playbackMode:"source",identity:"unverified browser capture"}};
    expect(hasExplicitSourceLock(captured)).toBe(false);
    expect(hasExplicitSourceLock(captured,{overridePlaylistTrackUrls:true})).toBe(false);
    expect(hasExplicitSourceLock({...captured,provenance:{...captured.provenance,manualFields:{playbackMode:true}}})).toBe(true);
    expect(hasExplicitSourceLock({...captured,provenance:{...captured.provenance,userSelectedSource:true}})).toBe(true);
    expect(hasExplicitSourceLock(captured,{selectedCandidate:candidate})).toBe(true);
    expect(hasExplicitSourceLock({...captured,provenance:{...captured.provenance,browserImport:false}})).toBe(true);
  });

  it("allows implicit embedded links and honors source versus lookup playback mode", () => {
    const exactYouTube = { ...candidate, provider: "youtube", id: "youtube:video" };
    expect(hasExplicitSourceLock({ ...track, provenance: { sourceKind: "audioSearch", audioCandidate: exactYouTube } })).toBe(false);
    expect(hasExplicitSourceLock({ ...track, provenance: { playbackMode: "source" } })).toBe(true);
    expect(hasExplicitSourceLock({ ...track, provider: "youtube", provenance: { playbackMode: "lookup", sourceKind: "audioSearch" } })).toBe(false);
  });

  it("keeps ordinary selected candidates pinned while honoring the lookup override", () => {
    expect(hasExplicitSourceLock({ ...track, provider: "import" }, { selectedCandidate: candidate })).toBe(true);
    expect(hasExplicitSourceLock({ ...track, provider: "youtube", provenance: { playbackMode: "lookup" } })).toBe(false);
  });
  it("keeps imported URLs recoverable unless a stronger lock exists", () => {
    const imported = {...track,provider:"import",provenance:{sourceKind:"audioSearch"}};
    expect(hasExplicitSourceLock(imported)).toBe(false);
    expect(hasExplicitSourceLock(imported,{overridePlaylistTrackUrls:true})).toBe(false);
    expect(hasExplicitSourceLock(imported,{overridePlaylistTrackUrls:true,selectedCandidate:candidate})).toBe(true);
    expect(hasExplicitSourceLock({...imported,provenance:{sourceKind:"audioSearch",playbackMode:"source"}},{overridePlaylistTrackUrls:true})).toBe(true);
    expect(hasExplicitSourceLock({...imported,provenance:{sourceKind:"youtubeRadio"}},{overridePlaylistTrackUrls:true})).toBe(true);
    expect(hasExplicitSourceLock({...imported,provenance:{sourceKind:"audioSearch",explicitSourceLock:true}},{overridePlaylistTrackUrls:true})).toBe(true);
    expect(hasExplicitSourceLock({...imported,playbackRange:{parentTrackId:track.id,chapterId:"c",sourceUrl:track.sourceUrl,startTime:0,endTime:20,parentTitle:"Set"}},{overridePlaylistTrackUrls:true})).toBe(true);
  });

});
