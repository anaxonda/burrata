# Remaining gaps in the top UI/download todo — 2026-10-21

This audit compares the first 13 items in `todo.md` with current code and available qualification evidence. It treats tests as implementation evidence, not live-use evidence. `search history` is the next todo item and is outside this top-13 cut.

## Concrete follow-up

- **Download fallback consistency:** Album/playlist batch uses configured order and the shared resolved match assessment (`src/App.svelte` `queueSelected`, `src/lib/batch.ts`). App tests exercise fallback and all 13 Bulgaria playlist tracks were acquired through the batch helper with live provider decisions. Exact Monochrome matches appeared as legacy `review` but shared-policy `accept` (`batch-source-decisions.json`), confirming why the batch path must use the shared assessment. The old `downloadTopMatch` helper was unused (no call sites) and has been removed; its legacy high-confidence gate was not active download behavior. Per-track downloads continue to use the source chooser for deliberate manual review. No forced real provider outage has verified configured fallback after a preferred-source miss end to end.

- **Full metadata on the existing downloads:** `bulgaria-downloads.json` verifies artist/title/album tags and duration for the 13 FLACs, but it records no ISRC, album-artist, label, genre, or date checks. Accepted Monochrome metadata now fills missing non-manual optional fields in `src-tauri/src/acquisition.rs`; `src-tauri/src/media.rs::merge_autotag_tags` now writes ISRC/catalog number. The native qualification log includes the new round-trip tests (1508 passed, 37 ignored). That proves implementation tests, not that these existing 13 files carry those fields: retag/rescan them with the final binary and verify actual tags. The current Monochrome results provide ISRC, but not all release metadata fields.

- **La Cubanita remains partial:** Physical tags and library data now verify title, artist, and album, with decoded audio unchanged (`la-cubanita-repair.json`). Catalog identity remains ambiguous across four recordings, so no unverified catalog choice should supply ISRC or other release fields. A safe next step is a reviewed exact recognition/crosswalk result or explicit manual metadata; until then this example does not establish complete tagging.

- **Fallback settings need a live failure case:** Settings persistence and provider order have unit/App coverage. The real Bulgaria checks found valid primary Monochrome matches, so they do not show the configured fallback activating after a real miss or failed inspection. Force a bounded preferred-provider miss on one track and record the selected configured fallback and final acquisition result.

- **Album download still lacks a live album pass:** `src/App.integration.test.ts` verifies that Download album queues all loaded release tracks through fallback without opening a chooser. The live Bulgaria playlist qualifies playlist batch acquisition, not the album-release navigation/button path. Verify one small album end to end after the batch fallback path is settled.

- **Playlist reorder/selection remain test-qualified:** App/component tests cover drag reorder, duplicate occurrence positions, inversion, and local-copy behavior for a protected provider-linked playlist. Current drag reorder is for local collections in source order; protected linked collections become a local copy and provider writeback remains paused. Dragging is only available for loaded/rendered rows, and is hidden under sorting/filtering. No live interaction was recorded. Confirm reorder/inversion on a local playlist in the app, and keep remote-provider order changes out of the claim unless explicitly qualified. Inversion currently applies to loaded occurrences/tracks; a paged playlist's unloaded remainder is outside that selection scope.

- **Custom online-source combinations are implemented, with bounded evidence:** `SearchPanel.svelte` accepts any enabled online source subset and persists it; `SearchPanel.test.ts` verifies a YouTube+SoundCloud combination and exclusion of a disabled provider. There is no live multi-source search qualification. Library mode remains separate from online combinations, which is a product boundary worth making explicit if “arbitrary” was intended to include library plus remote providers.

- **BPM/key accuracy is still open:** The documented synthetic detector checks and the real La Cubanita ambiguity audit are useful safety regressions, not a representative accuracy benchmark. A labeled, genre-diverse reference set is still needed before calling estimates accurate.

## Items with sufficient current evidence

- Full-screen Now Playing has an App-owned MCP capture and layout inspection; broader catalogs remain a follow-up.
- Album-to-playlist creation is covered by App/ReleaseView tests, including naming from the album and preserving loaded order. No separate live write evidence is recorded.
- Human-readable export names have native tests for Unicode filenames, portability, collisions, and ownership.
- Batch album/playlist selection avoids a chooser per track in tests and the live Bulgaria service-level run. The latter is not a trusted pointer-driven UI test.
- Queue clear/inverse controls and radio playlist capture have unit/App coverage; radio capture also has a disposable-profile live check in `radio-capture.json`.

## Evidence pointers

- [Qualification overview](README.md)
- [Bulgaria file checks](bulgaria-downloads.json)
- [Bulgaria shared-policy source decisions](batch-source-decisions.json)
- [La Cubanita repair](la-cubanita-repair.json)
- [Frontend full-suite log](/tmp/burrata-todo-information-full-tests.log)
- [Native metadata/tempo log](/tmp/burrata-todo-metadata-tempo-native-tests.log)
