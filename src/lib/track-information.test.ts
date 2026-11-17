import { describe, expect, it, vi } from "vitest";
import type { ArtistProfile, ArtistRef, ArtistSearchPage } from "./artist-types";
import type { Track } from "./types";
import { artistFromRecordingInformation, artistRecordingSearchQuery, createTrackArtistInformationLoader } from "./track-information";
import type { RecordingCatalogInfo, RecordingInformationResult } from "./recording-information";

const faithMb = { provider: "musicbrainz" as const, id: "ebcd0bad-75ef-4203-b7b3-7af38a9ef198" };
const faithDiscogs = { provider: "discogs" as const, id: "51271" };
function track(provenance: unknown = {}): Track {
  return { id: "track-1", provider: "youtube", providerId: "yt-1", sourceUrl: "https://youtube.test/watch?v=yt-1", title: "Song", artist: "Faith Evans", favorite: false, provenance };
}
function page(results: ArtistSearchPage["results"]): ArtistSearchPage {
  return { query: "Faith Evans", page: 1, pageSize: 20, hasMore: false, results, warnings: [] };
}
function result(artist: ArtistRef, name = "Faith Evans") {
  const sourceUrl = artist.provider === "musicbrainz" ? `https://musicbrainz.org/artist/${artist.id}` : artist.provider === "monochrome" ? `https://tracks.monochrome.st/artists/${artist.id}` : `https://discogs.com/artist/${artist.id}`;
  return { artist, name, sourceUrl };
}
function profile(artist: ArtistRef): ArtistProfile {
  return { artist: { ...result(artist) }, biography: "Profile bio", genres: [], tags: [], sourceTags: [], aliases: [], links: [], topTracks: [], releases: { artist, items: [], sourceCoverage: [], warnings: [] }, discogsAvailable: false, lastfmAvailable: false, sourceCoverage: [], warnings: [] };
}

describe("track artist information loader", () => {
  it("quotes and bounds artist recording search fields without allowing query injection", () => {
    expect(artistRecordingSearchQuery({...track(),title:'Song "title" \\ remix'},'Beck" OR artist:*')).toBe('recording:"Song \\"title\\" \\\\ remix" AND artist:"Beck\\" OR artist:*"');
    expect(artistRecordingSearchQuery({...track(),title:'"'.repeat(500)},'\\'.repeat(500)).length).toBeLessThan(512);
  });
  it("uses matching song credits to disambiguate a radio artist biography without selecting a recording", async () => {
    const beck = {provider: "musicbrainz" as const, id: "309c62ba-7a22-4277-9f67-4a162526d18a"};
    const homonym = {provider: "musicbrainz" as const, id: "1cc5adcd-1422-4b5c-a3cd-3ecd4f43f506"};
    const radio: Track = {...track(), id:"radio:mellow", provider:"radio", title:"Morning", artist:"Beck", album:"Morning Phase", provenance:{radioMetadata:{title:"Morning",artist:"Beck"}}};
    const recording: RecordingCatalogInfo = {provider:"musicbrainz",id:"11111111-2222-4333-8444-555555555555",title:"Morning",artist:"Beck",version:null,firstReleaseDate:null,durationSeconds:null,isrcs:[],genres:[],sourceUrl:"https://musicbrainz.org/recording/11111111-2222-4333-8444-555555555555",releases:[],recordingArtist:{name:"Beck",artist:beck}};
    const information: RecordingInformationResult = {resolution:"unresolved",recording:null,candidates:[{identity:"text_suggestion",recording},{identity:"text_suggestion",recording:{...recording,id:"22222222-2222-4333-8444-555555555555",version:"live"}}]};
    const lookupRecordingArtist = vi.fn(async (item: Track, name: string) => artistFromRecordingInformation(item,name,information));
    const loader = createTrackArtistInformationLoader({searchArtists:async()=>page([result(beck,"Beck"),result(homonym,"BECK")]),getArtistProfile:async artist=>profile(artist),lookupRecordingArtist});
    expect((await loader.load(radio))?.primaryArtist.artist).toEqual(beck);
    expect(lookupRecordingArtist).toHaveBeenCalledWith(expect.objectContaining({id:radio.id,title:"Morning",artist:"Beck",sourceUrl:""}),"Beck");
    expect(information.recording).toBeNull();
    expect(information.resolution).toBe("unresolved");
    expect((await loader.load({...radio,title:"Different song"}))).toBeNull();
    const conflicting = {...information,candidates:[...information.candidates,{identity:"text_suggestion" as const,recording:{...recording,recordingArtist:{name:"Beck",artist:homonym}}}]};
    expect(artistFromRecordingInformation(radio,"Beck",conflicting)).toBeNull();
    expect(artistFromRecordingInformation(radio,"Jeff Beck",information)).toBeNull();
    expect(artistFromRecordingInformation(radio,"Beck",{...information,candidates:[{identity:"text_suggestion",recording:{...recording,recordingArtist:null}}]})).toBeNull();
  });

  it("uses the current radio song's recognition ISRC and excludes station catalog identity", async () => {
    const lookupRecordingArtist = vi.fn(async () => faithMb);
    const loader = createTrackArtistInformationLoader({searchArtists:async()=>page([]),getArtistProfile:async artist=>profile(artist),lookupRecordingArtist});
    const station = {...track({artistCredits:[{name:"Station",artist:faithDiscogs}],radioMetadata:{title:"Song",artist:"Faith Evans",identifiers:{isrc:"us-bb4-05-81215"}}}),provider:"radio",isrc:"GBBAD0000000"};
    expect((await loader.load(station))?.primaryArtist.artist).toEqual(faithMb);
    expect(lookupRecordingArtist).toHaveBeenCalledWith(expect.objectContaining({isrc:"USBB40581215",sourceUrl:"",provenance:{recognitionIdentifiers:{isrc:"us-bb4-05-81215"}}}),"Faith Evans");
    expect(await loader.load({...station,title:"Different song"})).toBeNull();
    expect(await loader.load({...station,provenance:{}})).toBeNull();
    expect(lookupRecordingArtist).toHaveBeenCalledTimes(1);
  });

  it("uses a verified cached Spotify artist binding before name search", async () => {
    const lookupSpotifyArtist = vi.fn(async () => faithMb);
    const searchArtists = vi.fn(async () => page([]));
    const loader = createTrackArtistInformationLoader({lookupSpotifyArtist, searchArtists, getArtistProfile: async artist => profile(artist)});
    const spotifyId = "aaaaaaaaaaaaaaaaaaaaaa";
    const info = await loader.load(track({catalog:{verified:true,provider:"spotify",artists:[{provider:"spotify",id:spotifyId,name:"Faith Evans"}]}}));
    expect(lookupSpotifyArtist).toHaveBeenCalledWith(spotifyId);
    expect(info?.primaryArtist.artist).toEqual(faithMb);
    expect(searchArtists).not.toHaveBeenCalled();
  });
  it("deduplicates exact cross-provider matches and prefers a unique MusicBrainz identity", async () => {
    const searchArtists = vi.fn(async () => page([result(faithDiscogs), result(faithMb), result(faithMb)]));
    const getArtistProfile = vi.fn(async (artist: ArtistRef) => profile(artist));
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

  it("opens a provider-backed Monochrome artist ref from catalog provenance", async () => {
    const artist = { provider: "monochrome" as const, id: "123456789012345678" };
    const getArtistProfile = vi.fn(async (ref: ArtistRef) => profile(ref));
    const searchArtists = vi.fn(async () => page([]));
    const loader = createTrackArtistInformationLoader({ searchArtists, getArtistProfile });
    const info = await loader.load(track({
      artistCredits: [{ name: "Nipsey Hussle", role: "artist credit", artist }],
      catalog: { provider: "monochrome", artists: [{ provider: "monochrome", id: artist.id, name: "Nipsey Hussle" }] },
    }));
    expect(searchArtists).not.toHaveBeenCalled();
    expect(getArtistProfile).toHaveBeenCalledWith(artist);
    expect(info?.primaryArtist.artist).toEqual(artist);
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
