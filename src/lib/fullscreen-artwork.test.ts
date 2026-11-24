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

it("uses larger thumbnails from the selected YouTube video ID", () => {
  const source = "https://i.ytimg.com/vi/dQw4w9WgXcQ/hqdefault.jpg";
  expect(fullscreenArtworkUpgradeCandidates(track({ provider: "youtube", artworkUrl: source, provenance: {} }))).toEqual([
    { url: source.replace("hqdefault", "sddefault"), width: 640, height: 480 },
    { url: source.replace("hqdefault", "maxresdefault"), width: 1280, height: 720 },
  ]);
});

it("upgrades a custom playlist thumbnail to the same video's standard higher-resolution images", () => {
  const base = "https://i.ytimg.com/vi/_l1DbqVacBc/";
  const artworkUrl = `${base}hqdefault_custom_1.jpg?sqp=thumbnail-options&rc=unrecognized`;
  const upload = track({ provider: "youtube", providerId: "_l1DbqVacBc", artworkUrl, provenance: {} });
  expect(fullscreenArtworkUpgradeCandidates(upload)).toEqual([]);
  expect(fullscreenArtworkUpgradeCandidates({ ...upload, artworkUrl: artworkUrl.replace("rc=unrecognized", "rs=thumbnail-variant") })).toEqual([
    { url: `${base}sddefault.jpg`, width: 640, height: 480 },
    { url: `${base}maxresdefault.jpg`, width: 1280, height: 720 },
  ]);
});

it.each([
  ["https://f4.bcbits.com/img/a2324687513_3.jpg", "https://f4.bcbits.com/img/a2324687513_5.jpg", "https://f4.bcbits.com/img/a2324687513_10.jpg", 700, 1200],
  ["https://i1.sndcdn.com/artworks-nU2mhziz3vmX-0-large.png", "https://i1.sndcdn.com/artworks-nU2mhziz3vmX-0-t500x500.png", "https://i1.sndcdn.com/artworks-nU2mhziz3vmX-0-t1080x1080.png", 500, 1080],
  ["https://geo-media.beatport.com/image_size/95x95/cefa8af7-25b7-4172-af68-f6fc21bcc906.jpg", "https://geo-media.beatport.com/image_size/600x600/cefa8af7-25b7-4172-af68-f6fc21bcc906.jpg", "https://geo-media.beatport.com/image_size/1200x1200/cefa8af7-25b7-4172-af68-f6fc21bcc906.jpg", 600, 1200],
  ["https://geo-static.traxsource.com/scripts/image.php/52x52/38984d2afdd927eed907b8f5913ab18c.jpg", "https://geo-static.traxsource.com/scripts/image.php/600x600/38984d2afdd927eed907b8f5913ab18c.jpg", "https://geo-static.traxsource.com/scripts/image.php/1000x1000/38984d2afdd927eed907b8f5913ab18c.jpg", 600, 1000],
])("upgrades the same verified image asset for %s", (artworkUrl, larger, largest, largerSize, largestSize) => {
  const imported = track({ provider: "import", artworkUrl, provenance: {} });
  expect(fullscreenArtworkUpgradeCandidates(imported)).toEqual([
    { url: larger, width: largerSize, height: largerSize },
    { url: largest, width: largestSize, height: largestSize },
  ]);
  expect(fullscreenArtworkUpgradeCandidates({ ...imported, artworkUrl: largest })).toEqual([]);
});

it("upgrades selected MusicBrainz artwork even without a separate alternatives list", () => {
  const releaseId = "11111111-1111-4111-8111-111111111111";
  const artworkUrl = `https://coverartarchive.org/release/${releaseId}/front-500`;
  expect(fullscreenArtworkUpgradeCandidates(track({
    artworkUrl,
    provenance: { catalogProvider: "musicbrainz", releaseId },
  }))).toEqual([{ url: artworkUrl.replace("front-500", "front-1200"), width: 1200, height: 1200 }]);
});

it.each([
  "asset://localhost/local-cover.jpg",
  "https://f4.bcbits.com.evil.invalid/img/a2324687513_3.jpg",
  "https://f4.bcbits.com/img/a2324687513_3.jpg?signature=opaque",
  "https://i1.sndcdn.com/avatars-own-art-large.jpg",
  "https://i.ytimg.com/vi/not-a-video-id/hqdefault.jpg",
])("leaves local and unsupported artwork unchanged for %s", artworkUrl => {
  expect(fullscreenArtworkUpgradeCandidates(track({ artworkUrl, provenance: {} }))).toEqual([]);
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
