# Burrata 0.1.32

Draft release notes. Full native-suite, platform-package, installed-artifact, live-provider, and physical audio-output qualification is pending.

- Discogs artist-credit hydration now treats roleless catalog artist credits and `Primary Artist` consistently across Library reads and matching. It preserves uncertainty for `Unknown artist`, and reversed-caption results are accepted only when recording identity is corroborated.
- Radio favorites retain the canonical saved track ID and provider ID, and report save failures instead of silently targeting a different row.
- Artist and album browsing handles duplicate profiles and album families more clearly. Artist pages show source and year; album sorting and independent grid/list choices are available.
- Library bulk play and queue actions cover all matching pages, up to the 10,000 item bound, rather than only the currently loaded slice.
- Compact track rows use consistent buffering, downloaded, and missing-file states. Select-only rows without chord keys use buttons and remain keyboard operable.
- Exact Spotify contributor reads distinguish verified performers from composers and lyricists during playback matching, including legacy rows with freshly verified artist grouping. Identity conflicts and incomplete credits retain their protections; saved metadata is not automatically rewritten.
- Narrow album headers keep artwork, title and playback actions readable.
- Matching recognizes equivalent remaster spellings across dash, bracket, and parenthesis labels, and composes mastering labels with live or mix annotations. Years, performer evidence, unknown annotations, and conflicting recording versions remain part of validation.
- Sidebar icon recovery and Settings navigation are clearer, and the active radio station is highlighted. The compact radio bar shows the station name, and source table rows open their source link.
- Radio browsing uses the available width, and track information wraps long album names without covering controls.
- Fullscreen playback fits the hero artwork while keeping track details visible below the first fold.
- Shared selection controls include Queue selected. Album actions group playback, downloads, Soulseek searches, and local analysis more clearly.
- YouTube channel pages offer individual Videos and Playlists. Uploads use the shared play, queue, favorite, and download controls; uploader names remain separate from recording artists.
- Exact Bandcamp sources can play when saved duration is missing, after fresh page, track ID, title, artist, and stream identity checks. Opaque stream tokens are not retained in the warm audio cache.
- Missing Discogs durations can be checked against a complete independent Apple album programme without changing saved tags. Bounded artist-catalogue lookup can locate an edition omitted by song search; cleaned editions are rejected. A differing release date is permitted only within the same year when Flexible matching explicitly allows other releases; Exact and Balanced retain the date check.
- Isolated Chromium profiles receive their own native-host registration, and managed browser status detects an exited process.
- Optional song, concert, and set context lookup uses the configured AI connection and labels its results unverified. Optional search auto-queueing is off by default and never starts playback.
- Lyrics lookup handles identified official uploads with missing artist metadata. Waveform progress updates avoid rebuilding waveform bars.

## Qualification status and limits

- Native tests, candidate CI, installed Linux artifacts, and live-provider checks remain pending. Mac and Windows installed-artifact and physical-output checks are not yet qualified.
- Monochrome can still stall at its 15-second transport limit, and YouTube extraction has intermittent failures. This release does not claim those provider transport or extraction issues are fixed.
- Legacy v8 cache entries promoted into the Library can still have ambiguous recording identity. There is no automatic cleanup of those entries; playback remains subject to available identity evidence.
- A resolver or decoder result applies only to its exact recording and provider. It does not prove physical-device output or qualify another provider.
- The inspected Django restored-album alternatives omit required performer credits or lack a recording binding. Flexible mode continues to refuse those unverified sources; a famous title alone does not establish the requested recording.
