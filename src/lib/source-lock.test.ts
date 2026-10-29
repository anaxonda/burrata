import { describe, expect, it } from "vitest";
import { hasExplicitSourceLock, isCatalogSelection } from "./source-lock";
import type { Candidate, Track } from "./types";

const candidate = { id: "monochrome:12345678901234567", provider: "monochrome", url: "https://tracks.monochrome.st/track/12345678901234567" } as Candidate;
const track: Track = { id: "monochrome:12345678901234567", provider: "monochrome", providerId: "12345678901234567", sourceUrl: "https://tracks.monochrome.st/track/12345678901234567", artist: "Artist", title: "Song", favorite: false, provenance: {} };

describe("source lock classification", () => {
  it("keeps Monochrome and other catalog selections eligible for source fallback", () => {
    expect(isCatalogSelection(track)).toBe(true);
    expect(hasExplicitSourceLock(track, { selectedCandidate: candidate })).toBe(false);
    expect(hasExplicitSourceLock({ ...track, provenance: { sourceKind: "audioSearch" } }, { selectedCandidate: candidate, audioSearchResult: true })).toBe(false);
    expect(hasExplicitSourceLock({ ...track, provenance: { sourceKind: "audioSearch", audioCandidate: candidate } }, { audioSearchResult: true })).toBe(false);
    expect(hasExplicitSourceLock({ ...track, provider: "catalog", provenance: { catalogProvider: "spotify" } }, { selectedCandidate: candidate })).toBe(false);
  });

  it("locks explicit audio search rows, provider video rows, files, and ranged chapters", () => {
    expect(hasExplicitSourceLock({ ...track, provider: "import", provenance: { sourceKind: "audioSearch" } })).toBe(true);
    expect(hasExplicitSourceLock({ ...track, provider: "youtube", provenance: { sourceKind: "audioSearch" } })).toBe(true);
    expect(hasExplicitSourceLock({ ...track, localPath: "/music/upload.flac" })).toBe(true);
    expect(hasExplicitSourceLock({ ...track, playbackRange: { parentTrackId: track.id, chapterId: "c", sourceUrl: track.sourceUrl, startTime: 10, endTime: 20, parentTitle: "Set" } })).toBe(true);
  });

  it("keeps explicit lock markers authoritative even on catalog identities", () => {
    expect(hasExplicitSourceLock({ ...track, provenance: { sourceKind: "audioSearch", userSelectedSource: true } })).toBe(true);
    expect(hasExplicitSourceLock({ ...track, provenance: { explicitSourceLock: true } })).toBe(true);
  });

  it("pins exact embedded YouTube or SoundCloud uploads and honors source versus lookup playback mode", () => {
    const exactYouTube = { ...candidate, provider: "youtube", id: "youtube:video" };
    expect(hasExplicitSourceLock({ ...track, provenance: { sourceKind: "audioSearch", audioCandidate: exactYouTube } })).toBe(true);
    expect(hasExplicitSourceLock({ ...track, provenance: { playbackMode: "source" } })).toBe(true);
    expect(hasExplicitSourceLock({ ...track, provider: "youtube", provenance: { playbackMode: "lookup", sourceKind: "audioSearch" } })).toBe(false);
  });

  it("keeps ordinary selected candidates pinned while honoring the lookup override", () => {
    expect(hasExplicitSourceLock({ ...track, provider: "import" }, { selectedCandidate: candidate })).toBe(true);
    expect(hasExplicitSourceLock({ ...track, provider: "youtube", provenance: { playbackMode: "lookup" } })).toBe(false);
  });
});
