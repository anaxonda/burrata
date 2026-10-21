# Shared matching integration checkpoint

These are intermediate normal-profile Linux WebView observations from candidate native binary `b25ffaf6898b4d20e13d46a9c470865d02b04df0a3f2eba63c9994a6deed8a95`. They do not qualify a release or establish cross-platform reliability. Frontend development updates and further fixes followed this checkpoint; the final candidate must be tested again.

The original three chart entries, Baba Extended Mix, Nighttime in the Backyard, velib’bleu, To Miroloi Tis Manas, Diaraby with Ry Cooder, Turtledove, the Domna Samiou Μοιρολόι entry, and Cowboy with Billy Strings advanced their WebView media clocks without a media error. The recorded elapsed times include automation and polling; they are not first-PCM measurements or statistical startup benchmarks.

How Much Are They?, Matthew and Reckless failed identity selection; Mia Kokkini resolved identity but encountered a media network error. The old Pengus Victory Lap ID was unavailable in this profile, so that attempt is **not** evidence of a source-resolution failure and remains unqualified. New checks must regenerate the actual artist/search occurrence.

The full-track response hash changed for some successful plays. That hash includes hydrated display/context fields and is not a saved-tag comparison. Later radio checkpoints compare explicit title, artist, album, version, duration, ISRC and provider identity fields separately. The chart entries require a final explicit identity comparison.

The test app subsequently aborted after abandoned PyInstaller temporary extraction directories exhausted its temporary-storage quota. Old unreferenced extraction directories were removed. Per-command temporary-directory ownership, cancellation cleanup and bounded same-source media recovery are being qualified separately; those corrections must pass before final release acceptance.

## Corrected Linux candidate

The subsequent candidate `575c391f0642d250d32f416860f81f403366bdee98892a71c983bc7d0c5a4a49` played all 15 saved regression entries in the two `corrected-*-playback.json` reports. Each check required at least five seconds of advancing media time and compared saved recording identity fields before and after playback. These are Linux WebView observations with the native engine disabled, not first-PCM benchmarks or Mac/Windows qualification.

Matthew initially encountered a media network error in this run and recovered automatically; its first observed advancing playback was around 18 seconds after dispatch. A successful recovery does not establish fast or consistently reliable startup.

The exact Pengus “Level 2 - Miami (Insane)” occurrence remains unresolved. Apple catalog metadata identifies a 97,714 ms variant; public supported-provider searches have not located a verified playable equivalent. The base and Hard versions must not be substituted. Queue transitions, Bandcamp search UI, and final installed release checks remain pending. No public release is qualified by this checkpoint.

The generated-WAV native decoder contract and frontend production build passed after these playback observations. Final type checking reported zero errors and warnings.

Bandcamp search UI qualification subsequently passed: searching the published spelling “Lucas Santtana Night-time in the backyard” returned the original Bandcamp track, and clicking its search occurrence played more than six seconds with readyState 4 and no media error. The unhyphenated “Nighttime in the Backyard” query returned no results from Bandcamp's public API; this is a provider search limitation. See `bandcamp-search-playback.json`.

The final frontend suite at this checkpoint passed 1,088 tests across 87 files. A subsequent duration/seek correction requires renewed focused verification.

The frozen frontend's corrected three-chart queue qualification passed. Both automatic transitions were triggered by seeking near the authoritative source end, and all three subsequent entries advanced at least five seconds with readyState 4 and no media error. `corrected-chart-queue.json` records both HTML estimates and the stable published duration. An earlier run used a growing browser estimate and was invalid for end-of-track qualification; another overlapped frontend edits and was discarded. The six focused duration/seek/recovery tests and type check passed after the correction. Final installed cross-platform checks and the exact Pengus source remain pending.

## Cross-platform candidate follow-up

CI for `286fca6` exposed two regressions in the stable-duration correction on all three operating systems: chapter-end seek was clamped 50 ms early, and recovery without catalog/resolver duration could restart an exhausted stream. The correction preserves exact chapter endpoints and retains finite-media-bound validation for that legacy recovery case. The complete App integration suite subsequently passed 423 tests, and type checking passed. A fresh commit and cross-platform run are required; the failed run is not release qualification.

Final download review also found that Flexible alternatives could inherit requested album/ISRC tags. The follow-up replaces or clears automatic release identity fields using the actual source, keeps explicit manual overrides, and retains the requested identity separately in provenance. Native regression qualification is pending.

A live Flexible-mode check of the exact Pengus occurrence also failed to accept a source (`pengus-flexible-checkpoint.json`); the original matching setting was restored. This does not prove source absence. A public 110-second gameplay upload with “by Pengus” and “Insane Difficulty” in its caption exposes a possible title/credit normalization and discovery gap. Its duration is within Flexible tolerance; its recording relationship and actual audio remain unqualified. Further interpretation work is required before classifying this as unavailable or claiming playback success.

The same gameplay upload subsequently played successfully through Burrata's current resolver when explicitly selected from video search. Continued observation confirmed five seconds of advancing media time, readyState 4, a fully buffered 110-second source and no media error (`pengus-explicit-source-playback.json`). Thus public source playback is available; automatic catalog association and recording equivalence remain the gaps. The earlier standalone helper failure does not establish unavailability. No manual binding to the catalog entry was installed.

The corrected frozen native suite passes 1,456 tests (0 failed, 35 ignored), including the alternative-download tagging regression and the new contextual level/difficulty parsing cases. The parser can promote a plausible contextual candidate to fresh inspection; it never treats the “by” hint as verified artist evidence. The complete App suite passes 423 tests after the chapter/recovery correction. Renewed cross-platform CI and final-build playback qualification remain required.

A separate background experiment then acquired and decoded the exact gameplay upload using the production-equivalent VisionOS AAC handoff with a separate bounded 60-second audio-fetch stage. The 110.039-second candidate failed the existing normalized waveform comparison against the 29.929-second Apple Insane preview: best overall correlation was approximately 0.057, below the verifier's 0.96 threshold. This is not a foreground startup measurement and does not prove the tracks are unrelated under every audio transformation; it does mean the existing waveform verifier cannot establish recording equivalence. The gameplay upload remains playable when explicitly chosen, but automatic catalog binding is unqualified. See `pengus-background-audio-comparison.json`.

### Transport and cached-evidence follow-up

The b908387 live rerun passed 12 of 15 troublesome saved recordings; Baba encountered HTTP 403, Matthew HTTP 302, and Cowboy exhausted startup time after source access failures. These results supersede any inference that the earlier all-pass run established reliable repeatability. The follow-up permits one same-video AAC re-resolution after fresh identity verification for explicit redirect responses or 403, including freshly verified versioned recordings. It retains the original absolute startup deadline and does not follow redirects.

The cache audit also found that appending matching policy data to a strict six-field transport anchor invalidated new cache entries. Matching evidence now stays in its separately scoped record, with bounded evaluator metadata sufficient to reproduce the decision. A production-flow regression covers fresh shared-policy Topic verification, persistence, reconstruction, evidence restoration and cached promotion, including rejection under a different policy. The native suite passed 1,458 tests, with 35 ignored; live qualification of the rebuilt candidate remains pending.

The rebuilt transport candidate (SHA-256 `dcee5fef5ae80387c9d5a059c35a377949977cbd3f959b96eceab0e37830e014`) passed Baba, Matthew and Cowboy individually, then 13 of 15 cases in a complete live rerun. Diaraby stalled at direct FLAC startup and Mia at WebM/network recovery; these are unresolved transport-startup failures, so release qualification is not complete. All fifteen saved recording identity field sets remained unchanged. See `transport-candidate-playback.json`.
