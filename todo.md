# todo

- **PAUSED — extended Monochrome lossless integration (latest user instruction): The user paused this work because monochrome.samidy.com is currently down. Keep the new optional resolver route disabled; do not continue authorization/provider research or integration work until resumed. This supersedes the PROCEED instructions below for that work only. Continue the other playback, download, UI and release work. The extended integration is not a gate for the next release while paused.**

- **Historical PROCEED instruction — superseded by the current lossless pause:** The previous Monochrome implementation hold is lifted. Running agents may proceed from the [resolver plan](docs/monochrome-broader-resolver-plan.txt) and [backend findings](docs/monochrome-backend-alternatives-research.txt), coordinating existing file owners. Further provider/web research is paused. The requirement is working full-length lossless streaming, not metadata-only catalog access. Fresh provider authorization remains unverified; record that dependency explicitly and do not claim working playback or enable an unqualified route based only on the decoder probes. This instruction supersedes earlier HOLD/NO-GO implementation instructions in the linked documents.
- full screen mode (a la spotify)
- dl album doesnt work
- should be able to add album to new playlist with album name
- exported playlist should be human readable name
- dl playlist doesnt work? you can test dling the bulgaria choir playlist.
- does dl fallback preferences currently work?
- bpm/key not always accurate.
- should be able to select arbitary combinations of sources for online search.
- dled tracks should have full metadata tagging, with shazam if not complete, see 'la cubanita' dl that did not have full metadta. it seems auto tagging didnt work as advertissed
- for dling should not have to select source every time. should use default found audio dl source. so that album or playlist dl can just proceed through.
- should be able reorder any playlist items liek can be done in the queue
- should be able to clear queue, options to clear all, or right click and clear below. or ctrl/shift click for selection, and clear inverse. in general interacting with selections should be able to select inverse (not just queue)
- add radio option to auto-add items to playlist, able to choose playlist, default one with radio name. radio played items should be selectable, dlable just like other track list items.
- search history
- [ ] Running agents: implement and qualify the [track matching accuracy, robustness, and speed plan](docs/track-matching-improvement-plan.txt). Fix the reproduced parsing/evidence and three-mode policy defects, unify parsing/scoring, improve source/alias/cache reuse and extraction scheduling, and validate accuracy plus cold/warm latency. Coordinate shared-file ownership; record completion evidence and remaining qualification here.
- [ ] **PAUSED by the user; extended integration is excluded from the next release while paused.** When resumed, implement and qualify [lossless resolver integration](docs/monochrome-broader-resolver-plan.txt) using the existing evidence and [backend alternatives findings](docs/monochrome-backend-alternatives-research.txt). Prioritize typed resolver/transport boundaries, recording identity checks across all three matching modes, ephemeral media/key handling, native decoding, seek/refresh/cancellation, and explicit fallback reasons. Existing live Firefox responses and bounded Alvafro/Bacc decode probes establish feasibility, not fresh acquisition or end-to-end Burrata success. Keep unavailable provider access explicitly blocked rather than substituting metadata search as completion. Acceptance requires fresh authorized full-length lossless playback for the reported tracks plus held-out recordings, with seek/replay and queue checks. Do not resume open-ended provider research during this pause; coordinate shared-file ownership.
- for the album "9 months & 50 hours
July 30, 2026 • 14 tracks • 47 min
By
Fred again.. & LATIN MAFIA"
playback comes from youtube, yet it seems to be availabe on monochrome: "https://monochrome.samidy.com/album/apple/6796842304", I am able to play flac 16/44.1 from the monochrome website. so why do we fall back to lower quality youtube? is this a bug and can it apply to other tracks that there is actually a high quality audio version available but not being used?
-
-
## Current implementation progress — 2026-10-22

The extended Monochrome lossless integration remains paused and excluded from the next release. Public v0.1.5 remains the latest release; no v0.1.6 release commit or push has been made.

[Current goal and release evidence audit](docs/current-goal-audit-2026-10-21.md) records the current qualification boundary. The latest full native gate passed **1,688 tests, 0 failures, 37 ignored** (`/tmp/burrata-v016-optional-artwork-native.log`), including the response visibility and local tag-repair fingerprint fixes. The artwork-fix application build is qualified and its same-job album retry passes. Frontend tests pass **1,181 across 94 files**; browser companion tests pass **90**, and MCP tests pass **12**.

The eight Rumba source downloads now have actual files and completed jobs, including the eighth item’s later same-job retry. The evidence confirms file, saved-tag, decode, and fingerprint checks; the HTTP 403 retry’s triggering mechanism and any alternate-format recovery remain unproven. Six eligible files subsequently received local source-tag repairs. The repair evidence verifies expected artist/album tags in files and database, matching live-file/database hashes, unchanged full decoded PCM, preserved manual fields, and preserved recording identity. These six older repairs retain the hash-verified legacy browse path. New repairs now refresh registered-root fingerprints, with native regression coverage; a runtime rebrowse of the six older files is not claimed. See [Rumba source evidence](docs/performance-results/2026-10-21-ui-downloads/rumba-original-source-live-evidence.json) and [local repair evidence](docs/performance-results/2026-10-21-ui-downloads/local-source-tag-repair-live-evidence.json).

The Linux runtime seek check on build `ac2f229fe672aa6a0f8449b5b5b6d3e69c54649e543c1762c122c8a095e5edab` observed 1800→1803.85 seconds and 3500→3503.20 seconds with stable track identity and no media error. This is Linux evidence, not Mac qualification: [long-set seek report](docs/performance-results/2026-10-21-v015-mac-report/long-set-seek-v016-runtime.json).

The thirteen-track Soulseek album test now passes: thirteen jobs completed, no skipped tracks, one coherent lossless source folder, thirteen fully decodable FLAC outputs with verified exact-path asset hashes and available requested tags. Svatba required a same-job retry after the optional cover-art HTTP 500 fix. Existing asset publication provenance is preserved where reused; this is not an independent acoustic identity claim. [Final album evidence](docs/performance-results/2026-10-21-ui-downloads/soulseek-album-ac090bd2-live-evidence.json). The remaining bounded cross-mode and actual two-entry batch measurements are recorded in the [cross-mode report](docs/performance-results/2026-10-22-stage5-crossmode/README.md) and [runtime batch report](docs/performance-results/2026-10-22-stage5-crossmode/runtime-batch-observation.md). Remaining release work is source/release checks, cross-platform package qualification and publication. Pre-download ID remains a proposed follow-on; extended lossless is paused.

below this line is old work
------
## Active goal

Finish the current playback reliability/startup work, aim for starts below 12–15 seconds where possible, and publish a public release at a verified stable point. Preserve the normal library and sign-ins; keep one Burrata GUI. Playback and the queue may be discarded for testing, as explicitly authorized. Provider web players, mpv, mobile UI and further Spotify playlist writes are outside this effort.

## Release 0.1.5 candidate

- Cross-platform CI and installed-app audio smoke passed on Windows x64, macOS arm64 and Linux x64 for commit `7cb1fce`. Run fresh CI/installer qualification for the final compilation-excerpt source; the earlier build does not qualify subsequent changes.
- Build and qualify the actual installers, sign Firefox 0.2.3, package Chrome 0.2.3, verify all eight main-download checksums and publish the release with installation-guide links. Version 0.1.4 remains an older unpublished candidate tag; do not retag it.
- Record the final main-profile restoration and release provenance. The current source uses helper-free playback search by default, with ordinary helper fallback, and bounded AAC transport recovery for a freshly verified source denied with HTTP 403.

## Current implementation work

- Finish live qualification of the shared recording-identity/parsing work: identifier preservation and canonicalization, missing-duration radio ISRC lookup, complete credits, Unicode/title parsing, reusable complete-album timeline matching, and bounded preparation of saved radio songs. [Behavior and regression scope](docs/recording-identity.md).

- Finish release qualification of the integrated optional automatic Bandcamp streaming fallback. Metadata or cached identity evidence can authorize foreground playback; full audio-reference comparison runs separately after a failed match, with one bounded job and retry cooldown. Nighttime actual-library playback and fresh Bandcamp stream recovery now pass; retain those regressions.
- Promote the direct AAC YouTube resolver for eligible explicit/previously verified sources, retaining yt-dlp fallback and a runtime rollback switch. First-time catalog searches still require identity verification.
- Remove redundant external artist links from search results; use explicit Wikidata portraits and exact release-group cover fallbacks for MusicBrainz, with clean failed-image placeholders.
- Qualify the implemented three matching levels: Exact, Balanced (default), and Flexible. Shared typed identity evidence governs playback and downloads; source decisions retain policy/provenance, and identified alternatives retain their actual identity. Exact playback cannot silently consume a flexible-only decision.
- Add bounded background source preparation for saved/favorited radio identifications: retain exact Shazam/catalog locators currently discarded by the recognition parser, enrich identity, verify/cache sources, avoid work for unsaved listening, and retry unresolved items with cooldowns. Prefer recording ISRC or independently verified album crosswalks and recording-specific credit/title variants before audio comparison. Preserve radio history, tags, playback and external accounts.

## Current evidence

[Search comparisons, denied-source recovery and desktop checks](docs/performance-results/2026-10-20-helper-free-playback/README.md). The native search prototype passed all ten attempts across six reported recordings; its final default smoke retained a Billy Strings denial, which the subsequent AAC recovery resolves in two live attempts. Matthew and Pengus exploratory median first-PCM times improve from 13.9s to 9.0s and 8.6s to 7.2s. These are bounded Linux observations, not a p95 or physical-device claim.

The last completed native suite passed 1,477 tests (36 ignored); the frontend suite passed 1,097 tests across 87 files. Frontend type checking and production build pass. The evidence-only client-selection correction passes its regression and full suite; runtime qualification is next.

Twenty saved entries have sustained playback evidence across recent Linux WebView builds, with saved identity fields unchanged. A failed catalog data-track entry was automatically skipped and Arsenal then played. The fifteen principal entries and five additional occurrences must be rechecked as appropriate on the final frozen candidate. Bandcamp search/playback and the earlier natural chart-queue transitions pass. [Current qualification evidence](docs/performance-results/2026-10-20-shared-matching/README.md).

Pengus now plays from its original catalog entry in Flexible mode: an automatically verified 97.764-second soundtrack excerpt sustained playback with unchanged saved identity. The intermediate success required removing a confirmed missing-target test journal; automatic journal recovery, bounded transient-access retry, and final frozen-build playback are the remaining checks before release.

## Known source gaps and follow-up

- “Nighttime in the Backyard”: automatic Bandcamp fallback now plays from the real library, and explicitly selected label YouTube playback works. Automatic identity binding to that YouTube upload remains unqualified. Do not substitute the Tostex cover or drop guest checks. [Investigation](docs/performance-results/2026-10-20-nighttime-catalog/README.md).
- The first three Afro House Traxsource rows, Baba Extended Mix, velib’bleu and To Miroloi Tis Manas now pass actual automatic local playback. Automatic failed-entry advancement and shared album navigation have also been qualified; final frozen-build checks remain pending. Downloaded sample BPM is already analyzed, but both key estimations report no confident estimate.
- Pengus “Level 2 - Miami (Insane)”: the clean full-soundtrack source now yields a verified approximate excerpt in Flexible mode; it is hidden in Balanced and Exact. The noisy gameplay upload is not used for automatic association. Do not substitute the base or Hard version.
- Earlier Fruit Bats interruption and Ergo/Kaufman/choir source reports have not all been rechecked on this final runtime. Do not present the six-recording checks as exhaustive coverage.
- The reported middle-click opening ChatGPT was not reproduced by a trusted click here; complete external-browser behavior remains unconfirmed. No speculative opener change was installed.
- First-choice direct-player startup is being promoted for eligible known/verified sources; the current qualification binary includes that promotion. Optimized startup-versus-steady CPU comparison and broader physical macOS/Windows/network qualification remain follow-up; current debug/headless measurements do not answer them.
- Spotify profile totals can be inconsistent, so exhaustive profile playlist browsing is not established. Qobuz has no qualified working public backend. Browser sandbox variants and downloaded-file security prompts are not manually qualified on every system.

## Paused work and distribution

Spotify playlist work remains paused, preserving the prior verified existing-playlist addition and unknown creation outcomes. YouTube writeback stays off pending approval of its exact preview. No account or playlist writes are part of release checks.

Public v0.1.3 remains the published prerelease until the candidate gates pass. Main downloads include installers, both browser companions and checksums; supporting helper/source packages stay separate. Required helpers are bundled. [Installation guide](docs/install.md).

Last reconciled: 2026-10-20. [Historical detailed index](docs/todo-before-release015-2026-10-20.md).
