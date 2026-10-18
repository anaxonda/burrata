import { describe, expect, it } from "vitest";
import { activeChapter, chapterAbsolutePosition, chapterForRange, chapterRelativePosition, validChapterRange, virtualChapterTrack } from "./chapters";
import type { Candidate, Chapter, Track } from "./types";

const parent = { id: "recording-1", provider: "catalog", providerId: "mb-1", sourceUrl: "catalog-url", artist: "Album Artist", title: "Long Mix", favorite: false, provenance: { sourceKind: "catalog" } } as Track;
const candidate = { id: "candidate", provider: "youtube", url: "https://audio.test/video", title: "Full source", uploader: "DJ", durationSeconds: 1200, artworkUrl: null } as Candidate;
const chapter = { id: "chapter-1", title: "Opening", startTime: 120, endTime: 240, confidence: "high", suggestedTrack: { ...parent, artist: "Inferred Artist", title: "Opening" } } as Chapter;

describe("chapter playback helpers", () => {
  it("validates finite positive ranges bounded by candidate duration", () => {
    expect(validChapterRange(chapter, 240)).toBe(true);
    expect(validChapterRange({ ...chapter, endTime: 241 }, 240)).toBe(false);
    expect(validChapterRange({ ...chapter, startTime: NaN })).toBe(false);
  });

  it("creates stable range identity while retaining source provenance without catalog identifiers", () => {
    const first = virtualChapterTrack(parent, candidate, chapter)!;
    const second = virtualChapterTrack(parent, candidate, chapter)!;
    expect(first.id).toBe(second.id);
    expect(first.id).toBe(`chapter:${parent.id}:${chapter.id}`);
    expect(first.provider).toBe("chapter");
    expect(first.providerId).toBe(`${parent.id}:${chapter.id}`);
    expect(first.playbackRange).toMatchObject({ parentTrackId: parent.id, chapterId: chapter.id, sourceUrl: candidate.url, startTime: 120, endTime: 240 });
    expect(first.durationSeconds).toBe(120);
    expect(first.artist).toBe("Inferred Artist");
    expect(first.album).toBeNull();
    expect(first.inferredArtist).toBe("Inferred Artist");
    expect(first.isrc).toBeNull();
    expect(first.catalogNumber).toBeNull();
    expect((first.provenance as { audioCandidate: Candidate }).audioCandidate).toBe(candidate);
  });

  it("parses typographic chapter credits and clears parent release dates", () => {
    const track=virtualChapterTrack({...parent,releaseDate:"2024-01-01"},candidate,{...chapter,title:"QA Artist — First song",suggestedTrack:null})!;
    expect(track.artist).toBe("QA Artist");expect(track.title).toBe("First song");expect(track.releaseDate).toBeNull();
  });

  it("converts bounded chapter-relative and source-absolute positions", () => {
    const ranged = virtualChapterTrack(parent, candidate, chapter)!;
    expect(chapterAbsolutePosition(ranged, 25)).toBe(145);
    expect(chapterAbsolutePosition(ranged, 999)).toBe(240);
    expect(chapterRelativePosition(ranged, 145)).toBe(25);
    expect(chapterRelativePosition(ranged, 1)).toBe(0);
  });

  it("selects the chapter containing the source position", () => {
    expect(activeChapter([chapter], 120)).toBe(chapter);
    expect(activeChapter([chapter], 240)).toBeNull();
  });

  it("maps a native range to the exact canonical candidate chapter without title matching", () => {
    const renamedCandidateChapter = { ...chapter, title: "Different display title" };
    expect(chapterForRange([renamedCandidateChapter], { startTime: 120, endTime: 240 })).toBe(renamedCandidateChapter);
    expect(chapterForRange([renamedCandidateChapter], { startTime: 120, endTime: 241 })).toBeNull();
  });
});
