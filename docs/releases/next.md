# Burrata 0.1.28

Draft release notes. Full native-suite, platform-package, installed-artifact, live-provider, and physical audio-output qualification is pending.

- Discogs artist-credit hydration now treats roleless catalog artist credits and `Primary Artist` consistently across Library reads and matching. It preserves uncertainty for `Unknown artist`, and reversed-caption results are accepted only when recording identity is corroborated.
- Radio favorites retain the canonical saved track ID and provider ID, and report save failures instead of silently targeting a different row.
- Artist and album browsing handles duplicate profiles and album families more clearly. Artist pages show source and year; album sorting and independent grid/list choices are available.
- Library bulk play and queue actions cover all matching pages, up to the 10,000 item bound, rather than only the currently loaded slice.
- Compact track rows use consistent buffering, downloaded, and missing-file states. Select-only rows without chord keys use buttons and remain keyboard operable.
- Matching recognizes equivalent remaster spellings across dash, bracket, and parenthesis labels, and composes mastering labels with live or mix annotations. Years, performer evidence, unknown annotations, and conflicting recording versions remain part of validation.
- Sidebar icon recovery and Settings navigation are clearer, and the active radio station is highlighted. The compact radio bar shows the station name, and source table rows open their source link.
- Radio browsing uses the available width, and track information wraps long album names without covering controls.
- Fullscreen playback fits the hero artwork while keeping track details visible below the first fold.

## Qualification status and limits

- Native tests, candidate CI, installed Linux artifacts, and live-provider checks remain pending. Mac and Windows installed-artifact and physical-output checks are not yet qualified.
- Monochrome can still stall at its 15-second transport limit, and YouTube extraction has intermittent failures. This release does not claim those provider transport or extraction issues are fixed.
- Legacy v8 cache entries promoted into the Library can still have ambiguous recording identity. There is no automatic cleanup of those entries; playback remains subject to available identity evidence.
- A resolver or decoder result applies only to its exact recording and provider. It does not prove physical-device output or qualify another provider.
