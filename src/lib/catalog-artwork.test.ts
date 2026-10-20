import { expect, it } from "vitest";
import { musicBrainzCover, trackArtworkSources } from "./catalog-artwork";
import type { Track } from "./types";
const id="00000000-0000-4000-8000-000000000001";
it("derives covers only from exact MusicBrainz release identities", () => {
  expect(musicBrainzCover(`https://musicbrainz.org/release-group/${id}`)).toBe(`https://coverartarchive.org/release-group/${id}/front-250`);
  for (const url of [`https://musicbrainz.org.evil/release/${id}`,`https://musicbrainz.org/artist/${id}`,"https://musicbrainz.org/release/not-an-id",`https://user@musicbrainz.org/release/${id}`]) expect(musicBrainzCover(url)).toBeNull();
});
it("preserves edition artwork ahead of the exact release-group fallback", () => {
  expect(trackArtworkSources({artworkUrl:"https://example.test/edition.jpg",provenance:{catalogProvider:"musicbrainz",releaseGroupId:id}} as Track)).toEqual(["https://example.test/edition.jpg",`https://coverartarchive.org/release-group/${id}/front-250`,null]);
});
