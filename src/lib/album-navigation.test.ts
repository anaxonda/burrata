import { expect, it } from "vitest";
import type { Track } from "./types";
import { albumLookupArtist, directCatalogAlbumTarget } from "./album-navigation";
const track: Track = {id:"youtube:u_TSn1iO-hg",provider:"youtube",providerId:"u_TSn1iO-hg",sourceUrl:"https://music.youtube.com/watch?v=u_TSn1iO-hg",title:"Song",artist:"Main Act, Guest",album:"Album",favorite:false,provenance:{artistCredits:[{name:"Main Act"},{name:"Guest"}]}};

it("searches the album's primary structured artist instead of the track collaboration", () => {
  expect(albumLookupArtist(track)).toBe("Main Act");
  expect(albumLookupArtist({...track, albumArtist:"Album Artist"})).toBe("Album Artist");
  expect(albumLookupArtist({...track, albumArtist:"Unknown"})).toBe("Main Act");
  expect(albumLookupArtist({...track, provenance:{artistCredits:[{name:"Writer",role:"composer"},{name:"Main Act",role:"artist"}]}})).toBe("Main Act");
});
it("preserves atomic names and falls back intact when credit structure is unavailable", () => {
  expect(albumLookupArtist({...track,provenance:{artistCredits:[{name:"Earth, Wind & Fire"}]}})).toBe("Earth, Wind & Fire");
  for(const provenance of [{}, {artistCredits:[{name:"Main Act"},null]}]) expect(albumLookupArtist({...track,provenance})).toBe("Main Act, Guest");
});
it.each([['discogs','123','456'],['musicbrainz','6c56fb1d-e220-40ee-b836-10b6bf992ab7','d50b0808-1c23-43b2-b485-7248849268b2']])("opens the exact %s source release", (provider,releaseId,artistId) => {
  const catalog={...track,provider:"catalog",provenance:{catalogProvider:provider,releaseId,artistCredits:[{name:"Main Act",artist:{provider,id:artistId}}]}};
  expect(directCatalogAlbumTarget(catalog)?.release.release).toEqual({provider,id:releaseId});
  expect(directCatalogAlbumTarget({...catalog,provenance:{...catalog.provenance,releaseId:"../invalid"}})).toBeNull();
  expect(directCatalogAlbumTarget({...catalog,provenance:{...catalog.provenance,artistCredits:[{name:"Main Act",artist:{provider:"wrong",id:artistId}}]}})).toBeNull();
});
