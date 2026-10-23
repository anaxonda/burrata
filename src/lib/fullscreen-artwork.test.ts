import { describe, expect, it } from "vitest";
import { fullscreenArtworkUpgradeCandidates } from "./fullscreen-artwork";
import type { Track } from "./types";

const track = (overrides: Partial<Track> = {}): Track => ({
  id: "musicbrainz:recording",
  provider: "musicbrainz",
  providerId: "recording",
  sourceUrl: "https://musicbrainz.org/recording/recording",
  artist: "Artist",
  title: "Track",
  artworkUrl: "https://coverartarchive.org/release/11111111-1111-4111-8111-111111111111/front-250",
  favorite: false,
  provenance: {
    catalogProvider: "musicbrainz",
    releaseId: "11111111-1111-4111-8111-111111111111",
    releaseGroupId: "22222222-2222-4222-8222-222222222222",
    artworkAlternates: [
      "https://coverartarchive.org/release/11111111-1111-4111-8111-111111111111/front-250",
      "https://coverartarchive.org/release-group/22222222-2222-4222-8222-222222222222/front-250",
      "https://coverartarchive.org/release/33333333-3333-4333-8333-333333333333/front-250",
      "https://evil.example/release/11111111-1111-4111-8111-111111111111/front-250",
    ],
  },
  ...overrides,
});

describe("fullscreen artwork upgrades", () => {
  it("uses only the exact edition and its group, in that order", () => {
    expect(fullscreenArtworkUpgradeCandidates(track())).toEqual([
      { url: "https://coverartarchive.org/release/11111111-1111-4111-8111-111111111111/front-500", width: 500, height: 500 },
      { url: "https://coverartarchive.org/release/11111111-1111-4111-8111-111111111111/front-1200", width: 1200, height: 1200 },
      { url: "https://coverartarchive.org/release-group/22222222-2222-4222-8222-222222222222/front-500", width: 500, height: 500 },
      { url: "https://coverartarchive.org/release-group/22222222-2222-4222-8222-222222222222/front-1200", width: 1200, height: 1200 },
    ]);
  });

  it("keeps Spotify image URLs and dimensions from the matching catalog identity", () => {
    const spotify = track({
      provider: "spotify",
      providerId: "AAAAAAAAAAAAAAAAAAAAAA",
      artworkUrl: "https://i.scdn.co/image/small",
      provenance: {
        catalog: { provider: "spotify", providerId: "AAAAAAAAAAAAAAAAAAAAAA" },
        remoteCatalog: { images: [
          { url: "https://i.scdn.co/image/300", width: 300, height: 300 },
          { url: "https://i.scdn.co/image/1200", width: 1200, height: 1200 },
          { url: "https://i.scdn.co/image/no-dimensions" },
          { url: "https://evil.example/image", width: 1200, height: 1200 },
        ] },
      },
    });
    expect(fullscreenArtworkUpgradeCandidates(spotify)).toEqual([
      { url: "https://i.scdn.co/image/300", width: 300, height: 300 },
      { url: "https://i.scdn.co/image/1200", width: 1200, height: 1200 },
    ]);
    expect(fullscreenArtworkUpgradeCandidates(track({ provider: "spotify", providerId: "different" }))).toEqual([]);
  });
});

it("upgrades the exact selected YouTube image asset without changing its identity or options",()=>{
  const source="https://yt3.googleusercontent.com/SyntheticAsset_123=w120-h120-l90-rj";
  expect(fullscreenArtworkUpgradeCandidates(track({provider:"youtube",artworkUrl:source,provenance:{}}))).toEqual([
    {url:"https://yt3.googleusercontent.com/SyntheticAsset_123=w600-h600-l90-rj",width:600,height:600},
    {url:"https://yt3.googleusercontent.com/SyntheticAsset_123=w1200-h1200-l90-rj",width:1200,height:1200},
  ]);
  expect(fullscreenArtworkUpgradeCandidates(track({provider:"youtube",artworkUrl:source.replace("w120-h120","w1200-h1200"),provenance:{}}))).toEqual([]);
});
it.each([
  "https://yt3.googleusercontent.com.evil.invalid/Asset=w120-h120-rj",
  "https://user@yt3.googleusercontent.com/Asset=w120-h120-rj",
  "http://yt3.googleusercontent.com/Asset=w120-h120-rj",
  "https://yt3.googleusercontent.com/Asset=w120-h120-rj?token=secret",
  "https://yt3.googleusercontent.com/nested/Asset=w120-h120-rj",
])("does not rewrite an unrecognized or credential-bearing image URL %s",url=>{
  expect(fullscreenArtworkUpgradeCandidates(track({provider:"youtube",artworkUrl:url,provenance:{}}))).toEqual([]);
});
