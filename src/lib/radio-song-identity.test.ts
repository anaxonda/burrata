import { describe, expect, it } from "vitest";
import { radioSongTrackFromSnapshot, radioSongTrackIdentity } from "./radio-song-identity";
import type { Track } from "./types";

describe("radio song track identity", () => {
  it("projects a canonical listing ID into matching storage identity fields", () => {
    const id = `radio_song:${"a".repeat(64)}`;
    expect(radioSongTrackIdentity(id)).toEqual({
      id,
      provider: "radio_song",
      providerId: "a".repeat(64),
    });
  });

  it.each(["", "radio_song:", "radio_song:   ", "radio:recording", "radio_song:radio_song:abc", "radio_song:short"])(
    "rejects malformed listing ID %s",
    (id) => {
      expect(radioSongTrackIdentity(id)).toBeNull();
    },
  );

  it("prefers an exact canonical native track and preserves its recording identifiers", () => {
    const id = `radio_song:${"b".repeat(64)}`;
    const canonical: Track = {
      id,
      provider: "radio_song",
      providerId: "b".repeat(64),
      sourceUrl: "",
      artist: "Recognized Artist",
      title: "Recognized Song",
      isrc: "USABC2312345",
      favorite: false,
      provenance: {
        sourceKind: "radioSong",
        radioStationId: "station-a",
        recognitionIdentifiers: { isrc: "us-abc-23-12345", shazamId: "123" },
      },
    };
    const projected = radioSongTrackFromSnapshot({
      id,
      track: canonical,
      metadata: { stationId: "station-a", identifiers: { isrc: "us-abc-23-12345" } },
    });
    expect(projected).toEqual(canonical);
    expect((projected?.provenance as Record<string, unknown>).sourceKind).toBe("radioSong");
  });

  it("builds a safe canonical favorite projection for legacy snapshots without a track", () => {
    const id = `radio_song:${"c".repeat(64)}`;
    const projected = radioSongTrackFromSnapshot({
      id,
      stationName: "Radio One",
      metadata: {
        stationId: "station-a",
        artist: "  Recognized Artist ",
        title: " Recognized Song ",
        album: "Recognized Album",
        artworkUrl: "https://radio.example/cover.jpg",
        sourceUrl: "https://radio.example/live",
        metadataConfidence: "SongRec fingerprint",
        observedAt: "2026-11-02T12:00:00Z",
        identifiers: { isrc: "us-abc-23-12345", shazamId: "123" },
      },
    });
    expect(projected).toMatchObject({
      id,
      provider: "radio_song",
      providerId: "c".repeat(64),
      sourceUrl: "",
      artist: "Recognized Artist",
      title: "Recognized Song",
      album: "Recognized Album",
      isrc: "USABC2312345",
      favorite: false,
      provenance: {
        sourceKind: "radioSong",
        catalogVerified: false,
        radioStationId: "station-a",
        radioStationName: "Radio One",
        radioMetadataConfidence: "SongRec fingerprint",
        recognitionIdentifiers: { isrc: "us-abc-23-12345", shazamId: "123" },
      },
    });
    const serialized = JSON.stringify(projected);
    expect(serialized).not.toContain("radioStreamUrl");
    expect(serialized).not.toContain('"sourceKind":"radio"');
    expect(serialized).not.toContain('"sourceUrl":"https://radio.example/live"');
  });

  it.each([
    { id: `radio_song:${"d".repeat(64)}`, provider: "radio_song", providerId: "wrong", sourceUrl: "", artist: "A", title: "B", favorite: false, provenance: { sourceKind: "radioSong" } },
    { id: "radio_song:other", provider: "radio_song", providerId: "d".repeat(64), sourceUrl: "", artist: "A", title: "B", favorite: false, provenance: { sourceKind: "radioSong" } },
    { id: `radio_song:${"d".repeat(64)}`, provider: "radio", providerId: "d".repeat(64), sourceUrl: "https://radio.example/live", artist: "A", title: "B", favorite: false, provenance: { sourceKind: "radio", radioStreamUrl: "https://radio.example/live" } },
    { id: `radio_song:${"d".repeat(64)}`, provider: "radio_song", providerId: "d".repeat(64), sourceUrl: "", artist: "Other Artist", title: "B", favorite: false, provenance: { sourceKind: "radioSong", radioStationId: "different-station" } },
  ])("rejects a contradictory supplied canonical track", (track) => {
    expect(radioSongTrackFromSnapshot({
      id: `radio_song:${"d".repeat(64)}`,
      track,
      metadata: { stationId: "station-a", artist: "A", title: "B" },
    })).toBeNull();
  });

  it("rejects legacy fallback when radio metadata is missing or incomplete", () => {
    const id = `radio_song:${"e".repeat(64)}`;
    expect(radioSongTrackFromSnapshot({ id })).toBeNull();
    expect(radioSongTrackFromSnapshot({ id, metadata: { stationId: "station-a", artist: "A" } })).toBeNull();
  });
});
