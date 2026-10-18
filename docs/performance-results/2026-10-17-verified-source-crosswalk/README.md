# Verified recording crosswalk

The release goal includes source crosswalk implementation and verification. The implementation preserves the Spotify recording and saved tags, and uses verified recording-scoped artist aliases only for playback search and candidate validation.

## Identity evidence

Preparation reads the exact Spotify track and requires two complete identical album reads, including account, album, cursor, total and ordinal checks. MusicBrainz must have a unique direct relationship to that exact Spotify album URL. Complete Spotify and MusicBrainz release positions must align; title, version and duration checks then validate that anchored occurrence. Text similarity never selects another edition or recording.

The verified occurrence, raw MusicBrainz release and exact artist profiles are cached separately from signed playback URLs. The playback consumer reads caches only. It requires the current exact Spotify track-context cache, matching source fields and a unique album/track sidecar. Missing, ambiguous or conflicting context cannot be bypassed by old embedded provenance. A repeated complete album snapshot with duplicate or missing target IDs invalidates an older unique mapping.

A compact **Verify recording identity** action in the selected Spotify track's Details drawer performs the bounded read. It cancels on track change or drawer close, ignores stale results and offers an explicit Preview retry after success. It does not autoplay, change tags or write to provider playlists. The operation has a 30-second absolute deadline; ordinary playback retains its own existing 20-second deadline.

## Bounded verification

The combined native library suite passes **1,092 tests**, with 21 ignored. This includes exact URL/release HTTP fixtures, full-album alignment, version conflicts, wrong resource/edition rejection, duplicate/missing-target invalidation, stale embedded-context rejection and source-field preservation. The reachable Details panel and identity action pass 13 focused tests covering visibility, explicit retry, optional hints, cancellation and stale results. The complete final frontend suite passes 978 tests across 81 files. Production frontend build passes. Evidence is retained privately under `.cache/release-final-playback/`.

An earlier native run passed 1,086 tests and failed five profile tests because `/tmp` quota prevented fixture creation. After archiving an obsolete generated build trace, temporary-profile creation and the complete rerun pass. This was an environment failure, not five application regressions.

## Coverage limits and live acceptance

The public fixture Spotify album `6E4eD35ZrPXH2bF6LnPphg` has an exact MusicBrainz release link and aligns its 16 recordings, including track `0j8ulfrtVtBnyBr6p0CfFu`. The separately reported Spotify track `64rvuYgKLuIHqacdZZiwPr` belongs to album `7dYnr4klmbv4OTDceGfekf`; its exact MusicBrainz URL lookup returned 404. It must not inherit the other edition's mapping merely because its title and duration resemble it.

Live preparation succeeds on final native `4bfab35be85c` for exact Spotify track `0j8ulfrtVtBnyBr6p0CfFu` with album locator `6E4eD35ZrPXH2bF6LnPphg`, returning the expected MusicBrainz release/recording/track IDs and original Spotify fields. A wrong-edition locator for track `64rvuYgKLuIHqacdZZiwPr` is rejected because that exact ID is absent from the complete album. The initial no-hint REST track read was rate limited twice; no further REST retries were made. The hinted path uses independent complete account-backed album membership instead of trusting caller context. The action is visible in the actual Details panel at 952×1034. Exact 30-entry queue, occurrence IDs, settings and original paused position remain preserved.

A verified catalog crosswalk does not itself prove a playable source exists; no preview decoding claim is made for this fixture. No hardcoded recording exceptions or loose artist/title acceptance were introduced.
