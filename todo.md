# todo

## Completed release goal

[v0.1.5](https://github.com/anaxonda/burrata/releases/tag/v0.1.5) is published as a regular Latest release from frozen source `7b5a065714e061d9633a8c92905183fbefbb9542`. All eight public main downloads passed full-byte SHA-256 and size verification. Windows x64, macOS arm64 and Linux x64 CI and installed-app audio smoke passed. Chrome and signed Firefox companions are 0.2.3; required helpers are bundled. Version 0.1.4 remains an older unpublished candidate; it was not retagged.

Implemented shared identity/parsing and policy-scoped matching with Exact, Balanced (default) and Flexible levels; direct eligible YouTube playback with helper fallback and bounded AAC recovery; optional verified Bandcamp fallback and search; bounded saved-radio preparation; album navigation and consistent track rows; MusicBrainz image handling and search-link cleanup. [Identity behavior](docs/recording-identity.md).

Final native tests: 1,479 passed, 37 ignored. Frontend: 1,097 tests across 87 files, type checking and production build passed. Twenty-one saved entries sustained playback on the final Linux qualification build with saved identity unchanged. Two YouTube cases required retries; this is not a first-attempt reliability guarantee. Failed-entry advancement passed. Pengus automatic excerpt journal recovery and Flexible-only policy isolation passed. The normal library, original local assets and Spotify/YouTube sign-ins were preserved; Balanced was restored. [Final evidence and limitations](docs/performance-results/2026-10-20-shared-matching/README.md).

Pengus uses an automatically verified approximate 97.764-second soundtrack excerpt in Flexible mode. Its first background preparation is substantially slower than cached replay; no compilation scan is required for ordinary foreground playback.

## Known source gaps and follow-up

- “Nighttime in the Backyard”: automatic Bandcamp fallback now plays from the real library, and explicitly selected label YouTube playback works. Automatic identity binding to that YouTube upload remains unqualified. Do not substitute the Tostex cover or drop guest checks. [Investigation](docs/performance-results/2026-10-20-nighttime-catalog/README.md).
- The first three Afro House Traxsource rows, Baba Extended Mix, velib’bleu and To Miroloi Tis Manas now pass actual automatic local playback. Automatic failed-entry advancement and shared album navigation have also been qualified; final frozen-build playback checks passed. Downloaded sample BPM is already analyzed, but both key estimations report no confident estimate.
- Pengus “Level 2 - Miami (Insane)”: the clean full-soundtrack source now yields a verified approximate excerpt in Flexible mode; it is hidden in Balanced and Exact. The noisy gameplay upload is not used for automatic association. Do not substitute the base or Hard version.
- Earlier Fruit Bats interruption and Ergo/Kaufman/choir source reports have not all been rechecked on this final runtime. Do not present the six-recording checks as exhaustive coverage.
- The reported middle-click opening ChatGPT was not reproduced by a trusted click here; complete external-browser behavior remains unconfirmed. No speculative opener change was installed.
- First-choice direct-player startup is enabled for eligible known/verified sources. Optimized startup-versus-steady CPU comparison and broader physical macOS/Windows/network qualification remain follow-up; current debug/headless measurements do not answer them.
- Spotify profile totals can be inconsistent, so exhaustive profile playlist browsing is not established. Qobuz has no qualified working public backend. Browser sandbox variants and downloaded-file security prompts are not manually qualified on every system.

## Paused work and distribution

Spotify playlist work remains paused, preserving the prior verified existing-playlist addition and unknown creation outcomes. YouTube writeback stays off pending approval of its exact preview. No account or playlist writes are part of release checks.

Public v0.1.5 is the regular Latest release. Main downloads include installers, both browser companions and checksums; supporting helper/source packages stay separate. Required helpers are bundled. [Installation guide](docs/install.md).

Last reconciled: 2026-10-20. [Historical detailed index](docs/todo-before-release015-2026-10-20.md).
