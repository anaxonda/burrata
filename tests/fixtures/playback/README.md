# Playback reproduction fixtures

`kaval-spotify-artist-track.json` preserves the exact public Spotify artist-view row captured on 2026-10-16: track `64rvuYgKLuIHqacdZZiwPr`, duration **131 seconds**, with its original artist-scoped provenance and public source URL. It intentionally differs from the separately cached library runtime of 131.706 seconds. Reproduce the artist-view case without pinning a replacement source or changing default provider order. Catalog `available=true` does not establish playable fallback audio or an acoustic match; artist-page position 2 is not an album track number.

`kaufman-observed-unavailable-track.json` retains a matching actual MusicBrainz album queue occurrence and its no-playable-match/network-load failure. It does **not** fabricate provider availability=false, a Spotify album identity, missing provenance, or an occurrence entry ID. The historical failure does not establish its present cause or reproduce it by itself.

Both fixtures contain only public recording metadata and bounded failure observations. Credential/account envelopes, private queue/session exports, local media paths and signed/transient media URLs are excluded. Neither fixture verifies source equivalence or acoustic playback. Other JSON files in this directory retain their individual documented scopes.
