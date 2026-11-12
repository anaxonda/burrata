# Burrata 0.1.35

Release notes for the 0.1.35 candidate. Final installer checks run through the release workflow.

- Optional YouTube Music discovery inspects up to 20 catalog rows within the existing query and time budgets, retaining correctly credited recordings that appear after the first ten results.
- Exact MusicBrainz artist-profile aliases can match reordered flat performer credits when there is one complete, unambiguous mapping. Featured-credit roles, atomic band names, missing or extra performers, and fresh resolved-source validation keep their existing protections. Stored matching decisions use a new evaluator revision.
- Native startup recovery can reach its final source fallback if the refreshed stream URL is also denied before first audio. The original deadline, recovery cap, and explicit source locks remain enforced.
- Library album details keep their natural height so Load more follows the track list. Shared lists retain explicit accessibility semantics when their visual numbering is hidden.
- Windows builds can populate the Opus source cache from an independently checksum-verified mirror when the primary archive host is unavailable.

- Discogs artist-credit hydration now treats roleless catalog artist credits and `Primary Artist` consistently across Library reads and matching. It preserves uncertainty for `Unknown artist`, and reversed-caption results are accepted only when recording identity is corroborated.
- Radio favorites retain the canonical saved track ID and provider ID, and report save failures instead of silently targeting a different row.
- Artist and album browsing handles duplicate profiles and album families more clearly. Artist pages show source and year; album sorting and independent grid/list choices are available.
- Artist browsing omits unverified peer-filename artist guesses while retaining saved tracks and manual or catalog-backed identities. List cards wrap metadata in narrow windows, and the supported minimum window width is 350 pixels.
- Library bulk play and queue actions cover all matching pages, up to the 10,000 item bound, rather than only the currently loaded slice.
- Compact track rows use consistent buffering, downloaded, and missing-file states. Select-only rows without chord keys use buttons and remain keyboard operable.
- Exact Spotify contributor reads distinguish verified performers from composers and lyricists during playback matching, including legacy rows with freshly verified artist grouping. Identity conflicts and incomplete credits retain their protections; saved metadata is not automatically rewritten.
- Narrow album headers keep artwork, title and playback actions readable.
- Matching recognizes equivalent remaster spellings across dash, bracket, and parenthesis labels, and composes mastering labels with live or mix annotations. Years, performer evidence, unknown annotations, and conflicting recording versions remain part of validation.
- Flexible matching can inspect known remaster/restored-edition alternatives when other releases are allowed. Acceptance requires fresh metadata bound to the source ID, matching title and observed performer names, a known bounded duration, and no conflicting identity evidence. This does not identify the source as the requested master or establish exhaustive credits.
- Optional Music-catalog discovery keeps both canonical and permitted release-context queries within one research deadline. Matching duplicate source IDs preserves structured artist evidence; conflicting catalog claims remain uncertain. Partial provider results are retained without caching an incomplete search as complete.
- Verified canonical MusicBrainz credits no longer suppress optional lookup of provider-backed artist aliases. Alias lookup retains its existing time budget and does not invent spellings or relax performer checks.
- Sidebar icon recovery and Settings navigation are clearer, and the active radio station is highlighted. The compact radio bar shows the station name, and source table rows open their source link.
- Radio browsing uses the available width, and track information wraps long album names without covering controls.
- Fullscreen playback fits the hero artwork while keeping track details visible below the first fold.
- Shared selection controls include Queue selected. Album actions group playback, downloads, Soulseek searches, and local analysis more clearly.
- YouTube channel pages offer individual Videos and Playlists. Uploads use the shared play, queue, favorite, and download controls; uploader names remain separate from recording artists.
- Exact Bandcamp sources can play when saved duration is missing, after fresh page, track ID, title, artist, and stream identity checks. Opaque stream tokens are not retained in the warm audio cache.
- Missing Discogs durations can be checked against a complete independent Apple album programme without changing saved tags. Bounded artist-catalogue lookup can locate an edition omitted by song search; cleaned editions are rejected. A differing release date is permitted only within the same year when Flexible matching explicitly allows other releases; Exact and Balanced retain the date check.
- Isolated Chromium profiles receive their own native-host registration, and managed browser status detects an exited process.
- Ask AI appears only with a configured API connection. OpenRouter lookups research the actual song, concert, or set and show source links; other compatible endpoints provide model-knowledge context. Answers have separate headings, paragraphs, and attribution. Optional search auto-queueing remains off by default.
- Lyrics lookup handles identified official uploads with missing artist metadata. Waveform progress updates avoid rebuilding waveform bars.

- MusicBrainz artist recordings continue through all provider pages using raw response offsets, including pages containing malformed rows.
- Flexible release-context discovery prioritizes the structured primary performer over reordered group aliases. Requested MusicBrainz profiles are prepared before whole-release discovery, and enrichment is restricted to artists verified for the requested track occurrence; recording validation remains required.

- Browser companion 0.2.9 keeps YouTube uploaders and channel names as source attribution rather than artist metadata. New Beatport and Traxsource captures retain separate artist credits and provider IDs, including names containing punctuation.
- Artist and album grid cards contain long names and metadata without overlapping adjacent cards. The scrolling fullscreen Now Playing header sits flush against the top edge.

- Favorite hearts use a consistent filled state. Radio queues show detections from the current listening session above the current item.
- Saved track album links preserve their library membership. Legacy album groups can display related saved tracks without rewriting metadata; complete-album lookup opens an unambiguous catalog match or offers release choices.
- Now Playing artwork and titles keep separate layout space. AI context answers use readable plain-text formatting.

## Qualification status and limits

- Cross-platform CI and installer smoke checks passed on the preceding candidate; the final navigation update is subject to the release workflow checks. Linux packaged playback produced independent audio evidence across nine routes. Physical macOS and Windows audio output remain unverified; Windows output testing is deferred and is not a release blocker.
- A 10,000-track library loaded through all 49 pagination actions. The extended layout matrix and startup/CPU comparison are incomplete; no performance improvement is claimed.
- Monochrome can still stall at its 15-second transport limit, and YouTube extraction has intermittent failures. This release does not claim those provider transport or extraction issues are fixed.
- Legacy v8 cache entries promoted into the Library can still have ambiguous recording identity. There is no automatic cleanup of those entries; playback remains subject to available identity evidence.
- A resolver or decoder result applies only to its exact recording and provider. It does not prove physical-device output or qualify another provider.
- Live source35 resolver checks have found validated alternatives for Django’s “Minor Swing” and “Swing 42” under Flexible settings. These are alternative-release assessments, not verification of the requested restored mastering. A subsequent Swing 42 attempt passed identity checks but received HTTP 403 from the extracted media URLs. Packaged playlist attempts also encountered YouTube HTTP 403 responses. Provider access and incomplete performer evidence can still prevent playback; alternative-release playback does not identify a requested restored master.
