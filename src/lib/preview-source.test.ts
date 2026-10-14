import { describe, expect, it } from "vitest";
import { choosePreviewCandidate, isLinkedAudioSearchCandidate, plausiblePreviewCandidate, playlistUrlCandidate } from "./preview-source";
import type { Candidate, Track } from "./types";

const track: Track = {
  id: "track-1", provider: "local", providerId: "1", sourceUrl: "",
  artist: "Maya Jane Coles", title: "What They Say", favorite: false, provenance: {},
};
function candidate(provider: string, title: string, score: number, confidence = "review"): Candidate {
  return {
    id: `${provider}:1`, provider, url: `https://${provider}.test/1`, title,
    uploader: null, durationSeconds: 300, artworkUrl: null, formats: [],
    permission: "unknown", confidence, score, reasons: [], official: true,
  };
}

it("restores the playlist URL after a different stream has been remembered", () => {
  const imported = { ...track, provider:"import", sourceUrl:"https://www.youtube.com/watch?v=qt83jtmjClM", provenance:{ sourceKind:"audioSearch", playlistEntry:"https://www.youtube.com/watch?v=qt83jtmjClM" } };
  const remembered = { ...candidate("monochrome", "Sui Generis - Quizás, Porque", 0.97, "high"), url:"https://tracks.monochrome.st/track/174208180224667648" };
  expect(playlistUrlCandidate(imported, remembered)?.url).toBe(imported.sourceUrl);
  expect(playlistUrlCandidate(imported, remembered)?.provider).toBe("youtube");
  expect(playlistUrlCandidate({ ...imported, provenance:{sourceKind:"audioSearch",playlistEntry:"https://attacker.example/audio"}}, remembered)).toBeUndefined();
});

describe("automatic preview source matching", () => {
  it("matches non-Latin artist and song names", () => {
    const recording = { ...track, artist: "蔡琴", title: "被遗忘的时光" };
    expect(plausiblePreviewCandidate(recording, candidate("youtube", "蔡琴 - 被遗忘的时光", 0.9))).toBe(true);
  });
  it("accepts close title and artist matches but rejects obvious mismatches", () => {
    const close = candidate("youtube", "Maya Jane Coles - What They Say (Extended)", 0.79);
    const wrongArtist = candidate("youtube", "Other Artist - What They Say", 0.99);
    const wrongTrack = candidate("youtube", "Maya Jane Coles - Different Song", 0.99);
    expect(plausiblePreviewCandidate(track, close)).toBe(true);
    expect(plausiblePreviewCandidate(track, wrongArtist)).toBe(false);
    expect(plausiblePreviewCandidate(track, wrongTrack)).toBe(false);
  });

  it("accepts the exact linked audio-search source without catalog matching", () => {
    const searchTrack: Track = {
      ...track, artist: "Unknown artist", title: "Salaam",
      provenance: { sourceKind: "audioSearch" },
    };
    const exact = {
      ...candidate("youtube", "Bedouin, Hiya - Salaam (Francis Mercier Remix) - #afrohouse 🔥", 0.12, "low"),
      id: "abc123", url: "https://www.youtube.com/watch?v=abc123",
    };
    expect(plausiblePreviewCandidate(searchTrack, exact)).toBe(false);
    expect(isLinkedAudioSearchCandidate(searchTrack, exact)).toBe(true);
    expect(isLinkedAudioSearchCandidate(searchTrack, { ...exact, url: "https://attacker.example/abc123" })).toBe(false);
    expect(isLinkedAudioSearchCandidate(searchTrack, { ...exact, id: " " })).toBe(false);
    expect(isLinkedAudioSearchCandidate({ ...searchTrack, provenance: {} }, exact)).toBe(false);
  });

  it("requires verified identity for automatic Monochrome matching and an exact native link", () => {
    const mono = { ...candidate("monochrome", "Maya Jane Coles - What They Say", 0.99, "high"), id: "monochrome:155102871061270528", url: "https://tracks.monochrome.st/track/155102871061270528" };
    expect(plausiblePreviewCandidate(track, mono)).toBe(true);
    expect(plausiblePreviewCandidate(track, { ...mono, confidence: "review" })).toBe(false);
    expect(plausiblePreviewCandidate(track, { ...mono, url: "https://attacker.example/track/155102871061270528" })).toBe(false);
    const native = { ...track, id: mono.id, provider: "monochrome", providerId: "155102871061270528", provenance: { sourceKind: "audioSearch" } };
    expect(isLinkedAudioSearchCandidate(native, { ...mono, confidence: "review" })).toBe(true);
    expect(isLinkedAudioSearchCandidate(native, { ...mono, id: "monochrome:155102871061270529" })).toBe(false);
  });

  it("accepts an exact YouTube radio seed even when its catalog identity score is low", () => {
    const radioTrack: Track = { ...track, provenance: { sourceKind: "youtubeRadio" } };
    const seed = { ...candidate("youtube", "Different uploader title", 0.05, "low"), url: "https://youtu.be/abc123" };
    expect(plausiblePreviewCandidate(radioTrack, seed)).toBe(false);
    expect(isLinkedAudioSearchCandidate(radioTrack, seed)).toBe(true);
  });

  it("prefers the best plausible result within the requested provider and rejects low confidence", () => {
    const youtubeLow = candidate("youtube", "Maya Jane Coles - What They Say Radio Edit", 0.96, "low");
    const youtubeGood = candidate("youtube", "Maya Jane Coles - What They Say", 0.82);
    const soundcloudBetter = candidate("soundcloud", "Maya Jane Coles - What They Say", 0.99);
    expect(choosePreviewCandidate(track, [youtubeLow, soundcloudBetter, youtubeGood], "youtube")).toBe(youtubeGood);
    expect(choosePreviewCandidate(track, [youtubeGood, soundcloudBetter], "soundcloud")).toBe(soundcloudBetter);
  });
});
