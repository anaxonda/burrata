# Demand-driven aliases and radio identification — 2026-10-18

Post-v0.1.3 implementation and qualification checkpoint. These changes are not part of the published v0.1.3 installers.

## Implemented behavior

See [artist alias behavior](../../playback-artist-aliases.md). Alias discovery is demand-driven; retained artist evidence is reusable and recording-specific proof stays scoped. Per-request traces and an offline summary tool separate actual playback waiting from parallel metadata work.

Radio changes address repeated stale ICY titles overwriting audible identifications, unnecessary fixed-length capture waits, and a successful worker's channel closure being misreported as cancellation. Recognition tries three seconds first, then six/twelve only after no match, serially and with bounded spacing. Short samples are best effort, not guaranteed recognition. Current WebView recognition captures the playing output instead of reopening the station at a different broadcast offset.

Station history is available inline in 50-record pages. Filtering and sorting apply before saved-history pagination; station changes reject stale responses. Track rows share the compact playlist renderer. Duration bounds are hidden behind Filters; the popup escapes scrolling panes and flips above the playback bar. Narrow panes prioritize song titles over timestamps.

## Baseline and evidence limits

One old-build Radio Nova identification took **9,250 ms**, returning “Holidays” — Gonjasufi. Playback remained playing with the same station/session/source generation. A 35-second follow-up had no now-playing metadata and no observed Cassius reversion. This is a single recognition result without independent broadcast ground truth; it is not a latency distribution.

Live public MusicBrainz, Wikidata and Discogs data corroborate Lakis Chalkias / Lakis Halkias through shared Discogs identity. A recording lookup returned HTTP 503. The target Spotify recording and actual alias-assisted playback are therefore not claimed as verified.

## Verification

Deterministic checks:

- Native suite: **1,146 passed**, zero failed, 23 ignored. This includes final ISRC conflict guards and both stale station-tag races.
- Final frontend suite: **1,028 passed across 85 files**, zero failed. This includes shared action menus, station information and portal-based duration filters. History navigation previously took 11.50 seconds because the first 200 rows mounted repeatedly; virtualization above 50 rows reduced the same test to 4.12 seconds with its original ten-second timeout unchanged.
- Targeted identification integration: 19 passed. Progressive capture: 9 passed, including cancellation of in-flight recognition at timeout.
- MCP host: 12 passed. Alias trace report: 5 passed. Deterministic native WAV decoder contract passed.
- Svelte diagnostics: zero errors/warnings. Production frontend build passed.

[Machine-readable checks](deterministic-checks.json). [Debug cache microbenchmark](alias-cache-matching-debug.json): three warm-ups and 20 measured iterations, median **4.24 ms**, worst **7.15 ms**, CPU **0.093 seconds** across measured iterations. This is a synthetic local fixture, not a network or playback measurement. Release measurements were not run.

The first updated executable was launched through the verified Vite/MCP launcher. Queue entries and confirmed provider accounts/revisions were preserved. Three current-output Radio Nova checks returned “Zoblazo” — Ruthee in **4,068 / 4,371 / 4,112 ms**, with unchanged playback session/source generation. A follow-up at 83 seconds still had Ruthee, not Cassius. These are exploratory checks without independent broadcast ground truth, not a p95 claim. [Results](live-radio-exploratory.json).

A subsequent user screenshot exposed a separate station-card fallback: “Last identified” selected old station history even while the playbar and saved history held Ruthee. The card now receives current App metadata and keeps an independent, unfiltered latest fingerprint result. Later station timestamps and history filters cannot replace it with Cassius. Eleven RadioView tests pass, and a new normal-window screenshot shows Ruthee in the card and history. Identified cover-art preservation passed the final native build and live card/details/playbar check. A later identification returned Prince’s “Hot Thing” with cover art in 3,973 ms. [Artwork check](live-radio-artwork.json). An attempted build exhausted available disk space; only obsolete build artifacts were removed before retrying. User data and accounts were preserved.

Deterministic fixtures, cache microbenchmarks and live provider checks are distinct. A cache microbenchmark excludes HTTP, extraction, decoding and first-audio startup; it cannot establish complete playback p95.

## Desktop UI follow-up

Track, playlist/station, queue and chapter actions now share one ActionMenu frame with consistent layout, keyboard navigation and focus restoration. Radio history uses the same track menu, including an explicit unavailable Go to album action when recognition supplied no exact album identity. Saved tracks with verified Spotify album IDs navigate directly to the album.

Now Playing and the playbar display the radio station alongside the identified song. Station links are available through the shared outline information control. Identification implementation/confidence notices were removed from the station card. Duration filters are collapsed across album, artist, remote playlist, radio and download views; active bounds remain visible through the Filters indicator.

Actual Linux WebKit captures at 954×1036 verified the shared station info icon, unclipped station links and duration popup, plus Radio history with Now Playing open. Private screenshots remain local. One GUI, the current queue and paused Radio Nova state were preserved.
