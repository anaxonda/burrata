# todo

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
