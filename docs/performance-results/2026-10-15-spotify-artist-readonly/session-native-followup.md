# Spotify artist discography and album tracks: public-source follow-up

This follow-up closes the source-inspection gap recorded in [`README.md`](README.md): Spotify's current public desktop client does contain a complete offset/limit artist-discography reader. This qualifies the request shape and response envelope from source; it does not prove the persisted query will still be accepted at runtime or that the account's session can read every item. No credentials or authenticated requests were used.

## Pins inspected

The parent public web-player bundle is `https://open.spotifycdn.com/cdn/build/web-player/web-player.6b816267.js` (SHA-256 `1b9dcc42aeb9c4a7abfc673da946478b100d9fb62acb8bf7f27aa63ef8769602`; also listed in [`public-source-manifest.json`](public-source-manifest.json)). Its webpack chunk table pins the artist route chunk `xpui-routes-artist.8dd6925c.js`, fetched from `https://open.spotifycdn.com/cdn/build/web-player/xpui-routes-artist.8dd6925c.js`, 115,289 bytes, SHA-256 `b77a08cc02e87cb2cc0f6c8f7e2e613c5e0aa714d328963bef84dd2d81fc842b`. The bundle's artist-route components declare the operations below and wire actual list requests through the shared Pathfinder request client.

The same parent bundle defines the shared album query in `web-player.6b816267.js`, 4,474,090 bytes, SHA-256 as above. The album route chunk is `xpui-routes-album.7c466d0f.js`, URL `https://open.spotifycdn.com/cdn/build/web-player/xpui-routes-album.7c466d0f.js`, 32,090 bytes, SHA-256 `6ba66a92695bf45ca046ba9eb3f813931b775f12b895496ac7869aa884991012`. The relevant GraphQL query and mapper live in the parent bundle's shared modules, so the album chunk itself does not repeat the operation definition.

The Meld comparison source is pinned to commit `e0af26953c315e6d8dfcef25b6436f7747fb34c6`. Its `Spotify.kt` defines the Pathfinder URL and persisted-query request envelope; its `MeldSpotify.kt` implements the `getAlbum` query with URI, locale, offset, and limit. These private interfaces and hashes are undocumented and can rotate.

## Artist release paging

The live desktop artist route declares `queryArtistDiscographyAlbums`, `queryArtistDiscographySingles`, `queryArtistDiscographyCompilations`, `queryArtistDiscographyAll`, and `queryArtistDiscographyOverview`. All five declarations currently carry persisted-query SHA-256 `5e07d323febb57b4a56a42abbf781490e58764aa45feb6e3dc0591564fc56599`.

The four paged readers pass `variables: { uri, offset, limit, order }`. They return the category items from `data.artistUnion.discography.{albums|singles|compilations|all}.items`; category totals are read from the corresponding `totalCount` in the artist overview. The web client requests 20 entries per page and uses `offset` as the next page start. The sort control selects the `DateAsc`, `DateDesc`, `NameAsc`, and `NameDesc` order enum members.

Each item is a release-group-like entry. The current client unwraps each entry with `item.releases.items[0]` and drops entries without a first release. The resulting album/release object is consumed with `uri`, `name`, `date.year`, `coverArt.sources`, `type`, `artists.items` (`uri` and `profile.name`), and `sharingInfo`. The overview provides album/single/compilation/all totals and the artist identity. This is the missing native artist release pager; it is not a REST catalog fallback.

## Album and track metadata

The current desktop bundle declares both `getAlbum` and `queryAlbumTracks` with persisted-query SHA-256 `11c9cad9cb24c4f9e25ef9456f68621cfd6006a90f679e729c1d2f2fd70dfe16`. Its album loader sends `variables: { uri, locale, offset: 0, limit: 50 }` and reads `data.albumUnion`. The verified response fields used for album normalization are album `uri`, `name`, `type`, `date.isoString`, `artists.items`, `coverArt.sources`, `discs`, and `tracksV2.{items,totalCount}`. Each track wrapper is `{ track: ... }`.

The current client maps track metadata from `track.uri`, `name`, `artists.items` (`uri`, `profile.name`), `duration.totalMilliseconds`, `discNumber`, `trackNumber`, `playability.playable`, `contentRating.label`, `saved`, `playcount`, `relinkingInformation.linkedTrack.uri`, and `associationsV3.videoAssociations.totalCount`. This is sufficient to normalize real album tracks instead of only resolving track URIs. The same bundle separately defines `getAlbumNameAndTracks` (hash `8628ad33de3267d7bef516c76a746979a5f98891a2c9eaff3dfec828abdcd983`); Meld's helper sends URI/offset/limit and returns `tracksV2.pagingInfo.nextOffset`, but deliberately extracts only track URIs.

The checked web-client loader explicitly shows the first 50 detailed track items. The source also exposes `tracksV2.pagingInfo` through its paged URI helper, but this inspection did not find a full-metadata client loop that advances the `getAlbum` query's offset. Treat full-metadata paging via `getAlbum` as an implementation hypothesis to verify against a session response; keep the URI-only helper's `nextOffset` behavior source-verified and distinct.

## Implementation handoff

Use the web player's `queryArtistDiscographyAll` request first for an unfiltered artist release list, validate the `artistUnion` typename and `discography.all.items` array, unwrap `releases.items[0]`, and page with the observed offset/limit semantics. Use `getAlbum` for album metadata plus detailed first-page track rows; preserve `tracksV2.totalCount` and `pagingInfo` when present. Before shipping, runtime-qualify the current hash/request/body and actual account response using the authorized session, then test albums over 50 tracks to establish whether detail paging uses the same query. Do not silently pin this private hash as a durable public API.
