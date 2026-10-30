# UI consistency audit — 2026-10-30

This pass covers Settings, search and album navigation, album loading states, the library sidebar, track lists, Now Playing, track information, downloads, release views, automatic tagging, and playlist preferences. The changes focus on everyday controls, clear loading feedback, and keeping secondary detail available without dominating the screen.

## Changes

- Ordinary startup keeps the application visible with a small status indicator. Pending backup restoration still blocks interaction and initialization until restoration succeeds; failures retain their explanation and Retry action.
- Spotify album search results open the exact album view instead of entering playlist import. Back restores the search, and opening an album neither imports nor starts playback.
- Album cards show track loading and failures without requiring the Tracks section to be open. Play and Tracks remain visible; queueing, downloading, and playlist actions move into a compact More menu with outside-click and Escape dismissal.
- A single search provider shows supported result types. A previously selected unsupported type remains visible with an explanation, preserving search intent when changing providers.
- Settings puts routine playback controls first, moves engine/timing and optional audio analysis into disclosures, and places the compact player preference under Appearance. Simple preferences save individually; serialized saves prevent rapid edits from overwriting each other or saving an unrelated folder draft.
- Recording matching, volume adjustment, analysis, and source timing explanations use information popups. Popups stay inside clipping containers on narrow layouts and use the active theme surface for readable light/dark contrast. Matching remains an app-wide policy; its acceptance rules were not relaxed.
- Radio recognition saves immediately and rolls back its control on failure. Settings categories that do not need YouTube support no longer wait for that status request.
- AcoustID setup opens the actual connection settings. Local playlist copy controls are grouped, redundant Save actions and headings are removed, and playlist shortcut/export sections no longer interrupt one another.
- Tool configuration hides unused custom-path fields and duplicate links. Assistant setup has one location for its connection command and separate permission controls.

## Playback incidents investigated during the audit

The MusicBrainz album attempt spent almost its entire 20-second lookup window waiting for Monochrome. YouTube returned candidates earlier but none passed verification, and later sources were starved. The resolver now runs the ordered fallback chain alongside the preferred source, with only one fallback active at a time. Existing preference grace, shared deadlines, cancellation, strict matching, and the special Bandcamp-first path are preserved.

The “I Got Rhythm” failure exhibited the same fallback starvation. Its closest inspected YouTube candidate had an explicit Rome/1949 session suffix and a non-Topic uploader; close duration and matching words alone did not establish the requested recording. The matching policy remains unchanged.

The separate “After You've Gone” attempt started successfully, then the steady-playback watchdog reported no clock progress for 20 seconds following a seek. Its selected-source refresh timed out during extraction after eight seconds. The captured events and application log do not establish the original stall's cause. A bounded watchdog-expiry media sample now preserves clock, readiness, buffering, and source-generation evidence before recovery clears the source. It adds no polling or unbounded history. An isolated app reproduced a backward-seek stall; a standalone WebKit test using the same downloaded media file did not. Further isolation is needed before attributing it to a particular transport or decoder behavior.

## Validation

Focused integration tests cover exact album navigation/back, restoration safety, individual and rapid preference saves, timeout navigation into collapsed controls, AcoustID navigation, album loading/error states, and preserved search intent. Component tests cover radio saving/category loading, source fallback controls, tool configuration, analysis and volume settings, and playlist preferences. Native tests cover fallback scheduling configuration and diagnostic projections. Svelte checks passed with zero errors and warnings. Production frontend and native application builds passed. The final broad frontend run passed 1,747 of 1,748 tests; one existing native BPM integration case exceeded its five-second test timeout during the concurrent build. The unchanged test passed in isolation in 534 ms. Logs are retained with the local audit artifacts. The affected-suite run separately passed 649 tests, and eight native scheduling tests passed. Visual checks used a separate app/profile and covered Settings categories, narrow popup placement, and light/dark themes. The tests do not operate the user’s running player.

Tests and visual checks qualify the exercised states; they do not guarantee that an external provider has a verified, playable copy of every catalog recording. No public release is made by this audit.
