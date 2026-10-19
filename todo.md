# todo

## Current work

Public v0.1.3 is unchanged. The current implementation follows the approved playback reliability plan, with provider web players and mpv integration excluded. Preserve user data, credentials and queue; keep one Burrata GUI. Mobile UI and Spotify playlist writes remain out of scope.

- **Verification and source lookup:** fresh extractor identity and typed proof persistence are implemented, including the Matthew catalog path, fractional durations, identity-before-CDN validation, a shared two-extraction foreground budget and bounded optional research. Existing source IDs remain distinct from transient URLs. Live qualification of the final code remains outstanding.
- **Recovery and ownership:** typed failure policy, same-source refresh, configurable 20/30/45-second deadlines, radio reconnects, cancellation/account fences, native preload error retention and atomic queue checkpoints are implemented. Native range validation and recovery integration pass the 1,256-test native suite, including a repaired sparse-seek bug in stream-download.
- **Automated checks:** all 1,060 frontend tests pass; Svelte check reports zero errors and warnings. The native suite passes 1,256 tests with local loopback access; a final rerun includes the latest settings-navigation automation. The isolated FFmpeg fixture matrix passes 100/100 measured excerpts, 15 warmups and the expected corrupt-media failure. This measures decoder/fixture behavior, not live provider or audible playback reliability.
- **Matthew live smoke:** the production resolver produces five seconds of PCM, with exploratory first-PCM times around 12–14 seconds. It selects another upload; a short fingerprint comparison supports correspondence, but full-version equivalence and the five-second startup p95 target remain unqualified. Subsequent acoustic checks encountered provider HTTP 403; failures are retained.
- **Experiments:** the demand-driven YouTube.js/token worker, controlled decoder/range fixtures and machine-readable qualification reports are implemented. No alternative extractor or codec has been promoted. [Experiment instructions and evidence](docs/playback-reliability-experiments.md).
- **Remaining qualification:** final regression reconciliation; broader production-resolver checks for the reported recordings; desktop timeout-settings/playbar UI inspection; actual native/live comparisons; 30-minute memory/recovery runs; physical Apple Silicon Mac and Windows runtime acceptance. Do not mark these complete based on decoder-only checks.

Last reconciled: 2026-10-19 during playback reliability implementation. [Current implementation and verification evidence](docs/performance-results/2026-10-19-playback-reliability/README.md).

## Release status

[Burrata v0.1.3](https://github.com/anaxonda/burrata/releases/tag/v0.1.3) was published on 2026-10-18 at 20:29:42 UTC as a public prerelease from `9a45467bcff02103e2c84e000342660288b7ba3b`, with five installers, Chromium ZIP, AMO-signed Firefox 0.2.2 and checksums. Main/tag CI, all three platform release jobs and installed-app smokes pass. The eight main downloads and all 87 supporting assets match their checksum manifests; supporting downloads remain a draft. [Publication evidence](docs/performance-results/2026-10-18-todo-release/release-publication.json).

All six exact-source cases pass WebView media progression and native mixer progression, including paused seek/resume and both original Spotify Creep editions. Normal queue/account preservation and desktop layout checks pass. These are bounded live checks, not statistical performance qualification.

Preserve user data, credentials and queue state; keep one Burrata GUI. Mobile UI remains out of scope.

## Changes after v0.1.3

- **France Mac report:** artist-name album search and HTTPS radio transport fixes are implemented locally, with desktop artwork spacing corrected. Pengus native MP4 startup is improved: post-lookup first PCM fell from 8.05s to 0.11–0.27s and successful overall starts from 21.72s to 14.96–15.65s. One of four optimized cold attempts still hit the 20s deadline in YouTube extraction, so source-lookup latency was reproduced locally. Subsequent helper/extraction overlap passes three cold native runs at 12.2–12.8s to first PCM; one warmed full WebView run starts in 4.85s. These exploratory measurements do not establish p95 or French-network reliability. The original French Mac/WebView failure stage remains unverified. [Investigation](docs/performance-results/2026-10-18-france-macos/README.md).

- Completed alias/radio/album/menu/filter refinements are in the [task archive](task-archive.md#demand-driven-aliases-and-radio-fixes--2026-10-18); they are local post-release changes.
- **Unresolved recording binding:** “To Miroloi Tis Manas” has a strong full-length Lakis Halkias Topic candidate, but the imported Spotify recording still lacks a verified identity connection. Automatic fallback already ran. Do not weaken matching or hardcode the recording. [Investigation](docs/performance-results/2026-10-18-miroloi-match/README.md).

- The Lithium post-start recovery fix is implemented, passes 998 frontend and 1,114 native tests and is running in the normal window. It is **not included in the published v0.1.3 installers**; installer delivery remains for a future release. A later retry exposed an interrupted source transfer and HTTP 429 responses. Bounded backoff and resumed-audio progress checks are implemented; a live retry advances through 73.953s. Complete-recording and future provider availability are not claimed. [Evidence](docs/performance-results/2026-10-18-lithium-recovery/README.md).
- The public v0.1.3 extension downloads now use clear browser/version filenames and link to the new installation guide. Signed package bytes are unchanged; checksums and provenance were reconciled.

## Outstanding verification and known limitations

These are remaining checks or limitations, not a claim that every feature below is broken. Broader statistical qualification remains follow-up rather than an automatic release blocker.

- **Earlier playback reports:** recheck the recorded Fruit Bats native interruption and unresolved Ergo/Kaufman/choir source matches on the latest resolver. Do not label ambiguous or inaccessible recordings as fixed. The latest six-case checks do not close these older reports.
- **Spotify profile completeness:** browsing works, but inconsistent provider totals mean full playlist exhaustion is not established. This is a known limitation, not an unimplemented browsing feature.
- **Artwork:** the visible footer fallback is implemented and verified. The underlying asset-protocol failure remains an internal follow-up; it is not a remaining playback-bar layout fix.
- **Extension distribution:** source/icon staging and 88 extension tests pass; v0.1.3 now delivers the rebuilt Chromium and AMO-signed Firefox 0.2.2 companions. Chromium popup-to-native import is now verified in an isolated profile. Normal-profile Firefox popup reconnection is not claimed by that check.
- **Optional backends:** Qobuz source research is complete, but no tested public host supplied usable catalog JSON. An account-bound adapter is feasible; working Qobuz playback is not implemented or qualified.

## Deferred and follow-up

- Spotify playlist work remains paused. Preserve prior verified existing-playlist addition and unknown creation outcomes; no further writes or creation retries.
- YouTube writeback remains off pending approval of the exact saved preview; do not apply it as part of release checks.
- Optional broader live coverage: search continuation/radio, recognition, provider account expiry/switching, all-view imports, additional desktop states, and statistical performance/platform measurements. Implementations and existing tests are complete within the archived evidence; exhaustive coverage is not a new feature task or automatic release blocker. Codec/index experiments remain off.
- Some recordings may lack an accessible verified source. Keep version/identity checks and meaningful unavailable reasons; do not weaken matching or hardcode recordings. The exact reported Spotify Kaval edition has no linked MusicBrainz release in the captured lookup, unlike a separate fixture edition.
- Detailed historical checkpoints are retained in the [pre-release snapshot](docs/todo-before-final-release-2026-10-17.md); this file is the current task index. Preserve applicable limitations in release notes.

## Completed work

See the [task archive](task-archive.md) for completed items and their verification evidence.
