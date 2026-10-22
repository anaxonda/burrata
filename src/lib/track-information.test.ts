import { describe, expect, it, vi } from "vitest";
import type { ArtistProfile, ArtistSearchPage } from "./artist-types";
import type { Track } from "./types";
import { createTrackArtistInformationLoader } from "./track-information";

const faithMb = { provider: "musicbrainz" as const, id: "ebcd0bad-75ef-4203-b7b3-7af38a9ef198" };
const faithDiscogs = { provider: "discogs" as const, id: "51271" };
function track(provenance: unknown = {}): Track {
  return { id: "track-1", provider: "youtube", providerId: "yt-1", sourceUrl: "https://youtube.test/watch?v=yt-1", title: "Song", artist: "Faith Evans", favorite: false, provenance };
}
function page(results: ArtistSearchPage["results"]): ArtistSearchPage {
  return { query: "Faith Evans", page: 1, pageSize: 20, hasMore: false, results, warnings: [] };
}
function result(artist: typeof faithMb | typeof faithDiscogs, name = "Faith Evans") {
  return { artist, name, sourceUrl: artist.provider === "musicbrainz" ? `https://musicbrainz.org/artist/${artist.id}` : `https://discogs.com/artist/${artist.id}` };
}
function profile(artist: typeof faithMb | typeof faithDiscogs): ArtistProfile {
  return { artist: { ...result(artist) }, biography: "Profile bio", genres: [], tags: [], sourceTags: [], aliases: [], links: [], topTracks: [], releases: { artist, items: [], sourceCoverage: [], warnings: [] }, discogsAvailable: false, lastfmAvailable: false, sourceCoverage: [], warnings: [] };
}

describe("track artist information loader", () => {
  it("deduplicates exact cross-provider matches and prefers a unique MusicBrainz identity", async () => {
    const searchArtists = vi.fn(async () => page([result(faithDiscogs), result(faithMb), result(faithMb)]));
    const getArtistProfile = vi.fn(async (artist: typeof faithMb | typeof faithDiscogs) => profile(artist));
    const loader = createTrackArtistInformationLoader({ searchArtists, getArtistProfile });
    const information = await loader.load(track());
    expect(searchArtists).toHaveBeenCalledWith("Faith Evans");
    expect(information?.primaryArtist.artist).toEqual(faithMb);
    expect(getArtistProfile).toHaveBeenCalledWith(faithMb);
  });

  it("does not choose among same-provider homonyms", async () => {
    const searchArtists = vi.fn(async () => page([
      result(faithMb), result({ provider: "musicbrainz", id: "11111111-2222-4333-8444-555555555555" }),
    ]));
    const getArtistProfile = vi.fn(async () => profile(faithMb));
    const loader = createTrackArtistInformationLoader({ searchArtists, getArtistProfile });
    expect(await loader.load(track())).toBeNull();
    expect(getArtistProfile).not.toHaveBeenCalled();
  });

  it("uses a validated direct provenance ref without a name search", async () => {
    const searchArtists = vi.fn(async () => page([]));
    const getArtistProfile = vi.fn(async () => profile(faithMb));
    const loader = createTrackArtistInformationLoader({ searchArtists, getArtistProfile });
    const info = await loader.load(track({ artistCredits: [{ name: "Faith Evans", artist: faithMb, role: "main artist", joinPhrase: " & " }] }));
    expect(searchArtists).not.toHaveBeenCalled();
    expect(info?.artistCredits).toEqual([{ name: "Faith Evans", role: "main artist", artist: faithMb }]);
  });

  it("merges a catalog direct ref into the matching canonical credit and returns each track's own credits", async () => {
    const getArtistProfile = vi.fn(async () => profile(faithMb));
    const loader = createTrackArtistInformationLoader({ searchArtists: vi.fn(async () => page([])), getArtistProfile });
    const first = await loader.load(track({
      artistCredits: [{ name: "Faith Evans", role: "main artist", artist: null }],
      catalog: { artists: [{ provider: "musicbrainz", id: faithMb.id, name: "Faith Evans" }] },
    }));
    const second = await loader.load(track({ artistCredits: [{ name: "Faith Evans", role: "main artist", artist: faithMb }, { name: "Guest", role: "featuring" }] }));
    expect(first?.primaryArtist.artist).toEqual(faithMb);
    expect(second?.artistCredits.map(credit => credit.name)).toEqual(["Faith Evans", "Guest"]);
    expect(getArtistProfile).toHaveBeenCalledOnce();
  });

  it("retries after a transient profile failure", async () => {
    const searchArtists = vi.fn(async () => page([result(faithMb)]));
    const getArtistProfile = vi.fn().mockRejectedValueOnce(new Error("temporary network issue")).mockResolvedValue(profile(faithMb));
    const loader = createTrackArtistInformationLoader({ searchArtists, getArtistProfile });
    await expect(loader.load(track())).rejects.toThrow("temporary network issue");
    expect((await loader.load(track()))?.profile.biography).toBe("Profile bio");
    expect(searchArtists).toHaveBeenCalledTimes(2);
  });

  it("keeps provenance credit names intact instead of splitting raw artist strings", async () => {
    const searchArtists = vi.fn(async (name: string) => page([result(faithMb, name)]));
    const getArtistProfile = vi.fn(async () => profile(faithMb));
    const loader = createTrackArtistInformationLoader({ searchArtists, getArtistProfile });
    const original = track({ artistCredits: [
      { name: "Primary Artist", role: "main artist", artist: faithMb, joinPhrase: " feat. " },
      { name: "Featured Artist", role: "featuring", artist: null, joinPhrase: "" },
    ] });
    const info = await loader.load(original);
    expect(info?.artistCredits.map(credit => credit.name)).toEqual(["Primary Artist", "Featured Artist"]);
    expect(info?.primaryArtist.name).toBe("Primary Artist");
    const legacy = track();
    legacy.artist = "Primary Artist, Featured Artist";
    const legacyInfo = await loader.load(legacy);
    expect(searchArtists).toHaveBeenCalledWith("Primary Artist, Featured Artist");
    expect(legacyInfo?.artistCredits.map(credit => credit.name)).toEqual(["Primary Artist, Featured Artist"]);
  });
  it("uses recording identity when artist name lookup is inconclusive, without sharing it across songs", async () => {
    const lookupRecordingArtist = vi.fn().mockResolvedValueOnce(faithMb).mockResolvedValueOnce(null);
    const loader = createTrackArtistInformationLoader({
      searchArtists: vi.fn(async () => page([])),
      getArtistProfile: vi.fn(async () => profile(faithMb)),
      lookupRecordingArtist,
    });
    const first = track();
    expect((await loader.load(first))?.primaryArtist.artist).toEqual(faithMb);
    expect(await loader.load({...first, id: "other-song", title: "Another song"})).toBeNull();
    expect(lookupRecordingArtist).toHaveBeenCalledTimes(2);
  });

});
