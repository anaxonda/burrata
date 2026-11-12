import { expect, it } from "vitest";
import type { Track } from "./types";
import { albumLookupArtist, directCatalogAlbumTarget, freshSpotifyAlbumContext } from "./album-navigation";
const track: Track = {id:"youtube:u_TSn1iO-hg",provider:"youtube",providerId:"u_TSn1iO-hg",sourceUrl:"https://music.youtube.com/watch?v=u_TSn1iO-hg",title:"Song",artist:"Main Act, Guest",album:"Album",favorite:false,provenance:{artistCredits:[{name:"Main Act"},{name:"Guest"}]}};

it("restores only a validated album locator from the exact unchanged Spotify recording",()=>{
  const id="71fwMMqK8Kea3FguUUrl2t",albumId="7j1iNxlcdkJj864Bu0ETPj";
  const old={...track,id:`spotify:${id}`,provider:"spotify",providerId:id,sourceUrl:`https://open.spotify.com/track/${id}`,durationSeconds:235.542,provenance:{}};
  const fresh={...old,provenance:{catalog:{provider:"spotify",providerId:id,verified:true},recordingContext:{albumId,albumUri:`spotify:album:${albumId}`,trackNumber:3}}};
  expect(freshSpotifyAlbumContext(old,fresh)).toEqual({albumId,albumUri:`spotify:album:${albumId}`});
  for (const changed of [{...fresh,title:"Another song"},{...fresh,artist:"Another artist"},{...fresh,album:"Another album"},{...fresh,durationSeconds:240},{...fresh,providerId:"aaaaaaaaaaaaaaaaaaaaaa"},{...fresh,provenance:{...fresh.provenance,recordingContext:{albumId,albumUri:"spotify:album:aaaaaaaaaaaaaaaaaaaaaa"}}}]) expect(freshSpotifyAlbumContext(old,changed)).toBeNull();
  expect(freshSpotifyAlbumContext({...old,provenance:{manualFields:{title:true}}},fresh)).toBeNull();
});

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
it("uses the primary Spotify artist boundary for legacy album hints without splitting names", () => {
  const artists = [{provider:"spotify",id:"aaaaaaaaaaaaaaaaaaaaaa",name:"Earth, Wind & Fire"},{provider:"spotify",id:"bbbbbbbbbbbbbbbbbbbbbb",name:"Guest"}];
  const spotify = {...track,provider:"spotify",artist:"Earth, Wind & Fire, Guest",provenance:{catalog:{provider:"spotify",verified:true,artists}}};
  expect(albumLookupArtist(spotify)).toBe("Earth, Wind & Fire");
  expect(albumLookupArtist({...spotify,provenance:{catalog:{provider:"spotify",verified:true,artists:[...artists,null]}}})).toBe(spotify.artist);
});
it.each([['discogs','123','456'],['musicbrainz','6c56fb1d-e220-40ee-b836-10b6bf992ab7','d50b0808-1c23-43b2-b485-7248849268b2']])("opens the exact %s source release", (provider,releaseId,artistId) => {
  const catalog={...track,provider:"catalog",provenance:{catalogProvider:provider,releaseId,artistCredits:[{name:"Main Act",artist:{provider,id:artistId}}]}};
  expect(directCatalogAlbumTarget(catalog)?.release.release).toEqual({provider,id:releaseId});
  expect(directCatalogAlbumTarget({...catalog,provenance:{...catalog.provenance,releaseId:"../invalid"}})).toBeNull();
  expect(directCatalogAlbumTarget({...catalog,provenance:{...catalog.provenance,artistCredits:[{name:"Main Act",artist:{provider:"wrong",id:artistId}}]}})).toBeNull();
});
