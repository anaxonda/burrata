# todo

## Active goal

Finish the current playback reliability/startup work, aim for starts below 12–15 seconds where possible, and publish a public release at a verified stable point. Preserve the normal library, sign-ins and queue; keep one Burrata GUI. Provider web players, mpv, mobile UI and further Spotify playlist writes are outside this effort.

## Release 0.1.5 candidate

- Run fresh cross-platform CI on the final candidate. The previous run passed macOS/Windows; its Linux fixture publication race is corrected and the local final native suite passes 1,344 tests.
- Build and qualify the actual installers, sign Firefox 0.2.3, package Chrome 0.2.3, verify all eight main-download checksums and publish the release with installation-guide links. Version 0.1.4 remains an older unpublished candidate tag; do not retag it.
- Record the final main-profile restoration and release provenance. The current source uses helper-free playback search by default, with ordinary helper fallback, and bounded AAC transport recovery for a freshly verified source denied with HTTP 403.

## Current evidence

[Search comparisons, denied-source recovery and desktop checks](docs/performance-results/2026-10-20-helper-free-playback/README.md). The native search prototype passed all ten attempts across six reported recordings; its final default smoke retained a Billy Strings denial, which the subsequent AAC recovery resolves in two live attempts. Matthew and Pengus exploratory median first-PCM times improve from 13.9s to 9.0s and 8.6s to 7.2s. These are bounded Linux observations, not a p95 or physical-device claim.

The final default native suite passes 1,344 tests. Full frontend checks passed 1,074 tests before the final pause fix; all 411 App integration tests pass afterward. The actual muted WebView relayed source now passes active seek, paused seek and resume. Artist rows, album wrapping, duration parsing and chapter/search presentation are implemented and verified. Detailed completed checkpoints are in the [task archive](task-archive.md).

## Known source gaps and follow-up

- “Nighttime in the Backyard”: independent catalog duration now resolves, but the original label upload still has no automatically verified recording connection. Do not substitute the Tostex cover or drop guest checks. [Investigation](docs/performance-results/2026-10-20-nighttime-catalog/README.md).
- “To Miroloi Tis Manas”: the recorded Spotify edition still lacks a verified binding to the likely Lakis Halkias Topic source. Some recordings have incomplete, ambiguous or inaccessible provider metadata.
- Earlier Fruit Bats interruption and Ergo/Kaufman/choir source reports have not all been rechecked on this final runtime. Do not present the six-recording checks as exhaustive coverage.
- The reported middle-click opening ChatGPT was not reproduced by a trusted click here; complete external-browser behavior remains unconfirmed. No speculative opener change was installed.
- First-choice direct-player startup remains an opt-in experiment. Optimized startup-versus-steady CPU comparison and broader physical macOS/Windows/network qualification remain follow-up; current debug/headless measurements do not answer them.
- Spotify profile totals can be inconsistent, so exhaustive profile playlist browsing is not established. Qobuz has no qualified working public backend. Browser sandbox variants and downloaded-file security prompts are not manually qualified on every system.

## Paused work and distribution

Spotify playlist work remains paused, preserving the prior verified existing-playlist addition and unknown creation outcomes. YouTube writeback stays off pending approval of its exact preview. No account or playlist writes are part of release checks.

Public v0.1.3 remains the published prerelease until the candidate gates pass. Main downloads include installers, both browser companions and checksums; supporting helper/source packages stay separate. Required helpers are bundled. [Installation guide](docs/install.md).

Last reconciled: 2026-10-20. [Historical detailed index](docs/todo-before-release015-2026-10-20.md).
