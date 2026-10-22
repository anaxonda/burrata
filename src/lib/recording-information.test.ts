import { describe, expect, it, vi } from "vitest";
import type { Track } from "./types";
import { createRecordingInformationLoader, normalizeRecordingIsrc } from "./recording-information";

const recordingId = "b7500000-1111-4111-8111-111111111111";
const otherRecordingId = "c8500000-1111-4111-8111-111111111111";
const artistId = "a8500000-1111-4111-8111-111111111111";
const releaseId = "d8500000-1111-4111-8111-111111111111";
const isrc = "USBB40581215";
function track(provenance: unknown = {}, extras: Partial<Track> = {}): Track {
  return { id: "radio_song:b750", provider: "radio_song", providerId: "b750", sourceUrl: "https://radio.test/song", title: "Love Like This", artist: "Faith Evans", album: null, favorite: false, provenance, ...extras };
}
function row(id: string, options: { isrcs?: string[]; title?: string; version?: string; releases?: unknown[] } = {}) {
  return {
    id: `musicbrainz:${id}`, kind: "track", sourceId: "musicbrainz",
    track: { ...track(), provider: "musicbrainz", providerId: id, title: options.title ?? "Love Like This", version: options.version ?? null },
    data: {
      id, title: options.title ?? "Love Like This", disambiguation: options.version ?? "",
      "artist-credit": [{ name: "Faith Evans", joinphrase: "", artist: { id: artistId, name: "Faith Evans" } }],
      isrcs: options.isrcs ?? [], genres: [{ name: "R&B" }], length: 240000,
      releases: options.releases ?? [],
    },
  };
}

describe("recording information loader", () => {
  it("accepts only an echoed MusicBrainz recording ID and returns bounded release choices", async () => {
    const searchSource = vi.fn(async () => ({ results: [row(recordingId, { releases: [
      { id: releaseId, title: "Faithfully", date: "2005-04-05", country: "US", "artist-credit": [{ name: "Faith Evans", artist: { id: artistId } }] },
      { id: "invalid-id", title: "Bad ID", date: "2005", country: "US" },
      { id: otherRecordingId, title: "Missing country and credits", date: "2005-99", country: "USA" },
    ] })] }));
    const loader = createRecordingInformationLoader({ searchSource });
    const info = await loader.load(track({ catalog: { recordingId } }));
    expect(info.resolution).toBe("exact_recording_id");
    expect(info.recording?.id).toBe(recordingId);
    expect(info.recording?.recordingArtist).toEqual({ name: "Faith Evans", artist: { provider: "musicbrainz", id: artistId } });
    expect(info.recording?.releases).toEqual([
      expect.objectContaining({ id: releaseId, title: "Faithfully", date: "2005-04-05", country: "US", credits: [expect.objectContaining({ name: "Faith Evans" })] }),
      expect.objectContaining({ id: otherRecordingId, title: "Missing country and credits", date: null, country: null }),
    ]);
    expect(info.recording?.releases[1]).not.toHaveProperty("credits");
    expect(searchSource).toHaveBeenCalledWith(expect.objectContaining({ sourceIds: ["musicbrainz"], kinds: ["track"], limit: 50, query: "Faith Evans Love Like This" }));
  });

  it("does not accept a matching provider ID field when the search row ID fails to echo", async () => {
    const exact = row(recordingId);
    exact.id = "musicbrainz:other-recording";
    const loader = createRecordingInformationLoader({ searchSource: async () => ({ results: [exact] }) });
    const info = await loader.load(track({ catalog: { recordingId } }));
    expect(info.resolution).toBe("unresolved");
    expect(info.recording).toBeNull();
  });

  it("uses normalized raw ISRC membership and refuses a matching display field without raw evidence", async () => {
    const searchSource = vi.fn(async () => ({ results: [
      { ...row(recordingId), data: { ...row(recordingId).data as object, isrcs: ["US-BB4-05-81215"] } },
      { ...row(otherRecordingId), track: { ...row(otherRecordingId).track!, isrc: isrc } },
    ] }));
    const loader = createRecordingInformationLoader({ searchSource });
    const info = await loader.load(track({ recognitionIdentifiers: { isrc: "us-bb4-05-81215" } }));
    expect(normalizeRecordingIsrc("US-BB4-05-81215")).toBe(isrc);
    expect(info.resolution).toBe("exact_isrc");
    expect(info.recording?.id).toBe(recordingId);
    expect(info.candidates).toHaveLength(1);
    expect(searchSource).toHaveBeenCalledWith(expect.objectContaining({ isrc }));
  });

  it("leaves colliding ISRC rows unresolved and does not pick by ordering", async () => {
    const searchSource = vi.fn(async () => ({ results: [row(recordingId, { isrcs: [isrc] }), row(otherRecordingId, { isrcs: [isrc] })] }));
    const loader = createRecordingInformationLoader({ searchSource });
    const info = await loader.load(track({ recordingContext: { isrc } }));
    expect(info.resolution).toBe("ambiguous_isrc");
    expect(info.recording).toBeNull();
    expect(info.candidates.map(item => item.recording.id)).toEqual([recordingId, otherRecordingId]);
  });

  it("keeps text-only matches as suggestions without selecting them", async () => {
    const searchSource = vi.fn(async () => ({ results: [row(recordingId)] }));
    const loader = createRecordingInformationLoader({ searchSource });
    const info = await loader.load(track());
    expect(info.resolution).toBe("unresolved");
    expect(info.recording).toBeNull();
    expect(info.candidates).toMatchObject([{ identity: "text_suggestion", recording: { id: recordingId } }]);
  });

  it("rejects contradictory title or version even when the ISRC matches", async () => {
    const searchSource = vi.fn(async () => ({ results: [
      row(recordingId, { isrcs: [isrc], title: "Different Song", version: "Studio" }),
      row(otherRecordingId, { isrcs: [isrc], version: "Live" }),
    ] }));
    const loader = createRecordingInformationLoader({ searchSource });
    const info = await loader.load(track({ recognitionIdentifiers: { isrc } }, { version: "Studio" }));
    expect(info.resolution).toBe("unresolved");
    expect(info.recording).toBeNull();
  });

  it("retries after a transient provider error", async () => {
    const searchSource = vi.fn().mockRejectedValueOnce(new Error("temporary network issue")).mockResolvedValue({ results: [row(recordingId)] });
    const loader = createRecordingInformationLoader({ searchSource });
    await expect(loader.load(track({ catalog: { recordingId } }))).rejects.toThrow("temporary network issue");
    expect((await loader.load(track({ catalog: { recordingId } }))).resolution).toBe("exact_recording_id");
    expect(searchSource).toHaveBeenCalledTimes(2);
  });
});
