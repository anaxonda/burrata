# todo

## Current work

No remaining items in the current release scope. Completed work is in the [task archive](task-archive.md#published-v013--2026-10-18). Deferred work and known limitations remain separate below.

Last reconciled: 2026-10-18 after publication.

## Release status

[Burrata v0.1.3](https://github.com/anaxonda/burrata/releases/tag/v0.1.3) was published on 2026-10-18 at 20:29:42 UTC as a public prerelease from `9a45467bcff02103e2c84e000342660288b7ba3b`, with five installers, Chromium ZIP, AMO-signed Firefox 0.2.2 and checksums. Main/tag CI, all three platform release jobs and installed-app smokes pass. The eight main downloads and all 87 supporting assets match their checksum manifests; supporting downloads remain a draft. [Publication evidence](docs/performance-results/2026-10-18-todo-release/release-publication.json).

All six exact-source cases pass WebView media progression and native mixer progression, including paused seek/resume and both original Spotify Creep editions. Normal queue/account preservation and desktop layout checks pass. These are bounded live checks, not statistical performance qualification.

Preserve user data, credentials and queue state; keep one Burrata GUI. Mobile UI remains out of scope.

## Changes after v0.1.3

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
