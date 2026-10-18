# todo

## Current work for v0.1.3

- Complete integrated checks, package/re-sign the updated companions and installers, then publish v0.1.3. Move these remaining entries to the archive only after their verification and publication.

Last reconciled: 2026-10-18. Completed implementations and their bounded evidence are archived; incomplete verification remains listed below.

## Release status

[Burrata v0.1.2](https://github.com/anaxonda/burrata/releases/tag/v0.1.2) was published on 2026-10-18 at 16:16 UTC as a public prerelease, with five installers, both browser extensions and checksums. Release creation is complete. GitHub status was checked directly on 2026-10-18; older draft/hold descriptions in historical reports are superseded.

## Changes after v0.1.2

- Completed local changes for v0.1.3 are recorded in the [task archive](task-archive.md#completed-implementation-for-v013--2026-10-18). They are newer than the published v0.1.2 commit `3450772`; delivery remains part of the current release task.
- All six exact-source cases pass both WebView media progression and native mixer progression, including paused seek/resume and both original Spotify Creep editions. Normal queue/account preservation and desktop layout checks pass; see the archive and retained reports.

Preserve user data, credentials and queue state; keep one Burrata GUI. Mobile UI remains out of scope.

## Outstanding verification and known limitations

These are remaining checks or limitations, not a claim that every feature below is broken. Broader statistical qualification remains follow-up rather than an automatic release blocker.

- **Earlier playback reports:** recheck the recorded Fruit Bats native interruption and unresolved Ergo/Kaufman/choir source matches on the latest resolver. Do not label ambiguous or inaccessible recordings as fixed. The latest three-track checks do not close these older reports.
- **Spotify profile completeness:** browsing works, but inconsistent provider totals mean full playlist exhaustion is not established. This is a known limitation, not an unimplemented browsing feature.
- **Artwork:** the visible footer fallback is implemented and verified. The underlying asset-protocol failure remains an internal follow-up; it is not a remaining playback-bar layout fix.
- **Extension distribution:** source/icon staging and 88 extension tests pass; the pending v0.1.3 release must deliver rebuilt/re-signed companions. Chromium popup-to-native import is now verified in an isolated profile. Normal-profile Firefox popup reconnection is not claimed by that check.
- **Optional backends:** Qobuz source research is complete, but no tested public host supplied usable catalog JSON. An account-bound adapter is feasible; working Qobuz playback is not implemented or qualified.

## Deferred and follow-up

- Spotify playlist work remains paused. Preserve prior verified existing-playlist addition and unknown creation outcomes; no further writes or creation retries.
- YouTube writeback remains off pending approval of the exact saved preview; do not apply it as part of release checks.
- Optional broader live coverage: search continuation/radio, recognition, provider account expiry/switching, all-view imports, additional desktop states, and statistical performance/platform measurements. Implementations and existing tests are complete within the archived evidence; exhaustive coverage is not a new feature task or automatic release blocker. Codec/index experiments remain off.
- Some recordings may lack an accessible verified source. Keep version/identity checks and meaningful unavailable reasons; do not weaken matching or hardcode recordings. The exact reported Spotify Kaval edition has no linked MusicBrainz release in the captured lookup, unlike a separate fixture edition.
- Detailed historical checkpoints are retained in the [pre-release snapshot](docs/todo-before-final-release-2026-10-17.md); this file is the current task index. Preserve applicable limitations in release notes.

## Completed work

See the [task archive](task-archive.md) for completed items and their verification evidence.
