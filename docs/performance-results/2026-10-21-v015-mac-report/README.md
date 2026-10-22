# v0.1.5 playback report from macOS

The user supplied photographs of the released macOS app. Local investigation runs on Linux; reproducing a shared resolver failure does not establish or exclude a separate macOS media problem. This work is unreleased.

## Latest qualification — 2026-10-22

This section supersedes the dated snapshots below. The full native suite passes 1,688 tests (37 ignored), frontend 1,181 tests across 94 files, and player contracts 5/5. Saved-library Bacc, Vibe With Me, Sacrifices and Miroloi have successful one-attempt Linux playback observations on the recorded follow-up builds. [Saved-library evidence](saved-library-v8-runtime.json) and the current goal audit retain build-specific limits.

The newer [long-set seek report](long-set-seek-v016-runtime.json) records Rooftop at 1800→1803.85 seconds and CDMX at 3500→3503.20 seconds, playing with stable queue identity on build `ac2f229f…`. These are Linux runtime observations, not physical macOS audibility or hardware qualification. Persistent buffering, artist suggestions and pointer-capture cleanup are covered by frontend tests. Final cross-platform installer smoke is still pending the v0.1.6 release workflow.

## Historical live seek qualification (2026-10-21)

On native binary SHA256 `8f9f79cb219ae031d0e626bd7b2bcd59407cec0d81e85745a68ad1e5b93641c6`, using the normal profile and authenticated app MCP controls:

- Rooftop `youtube:6MAzUT1YhWE`: automatic WebView-to-native handoff at 1800 seconds succeeded; native first audio 2286 ms after fallback started. Queue occurrence IDs survived, and natural end advanced to the saved Bacc entry, which played through its end.
- After process restart (default WebView), Full Mixtape `youtube:mDtud5fLgFQ`, duration 4151 seconds: startup succeeded; seeking to the reported 278-second point handed off to native and continued beyond 292 seconds without a failure. Native first audio 5395 ms after fallback began. Entry ID `8d2394fb-2fbe-45a0-a7f8-d4c156b966e1` stayed unchanged.
- A subsequent native seek to 3500 seconds completed with first audio in 649 ms and sustained progress beyond 3542 seconds, same source and queue entry.

A separate normal-profile process restart then played saved Library ID `catalog:discogs:46a5e4250d2cac2a720d5133d6b0d8386e48de8aeaea4224e2b94e03e9e60a47` directly through `play_tracks_queued`, before any discography query. The default WebView route reached 25.99 seconds of a 198-second recording, no media failure. Evidence: `/tmp/burrata-bacc-restart-response.json` and `/tmp/burrata-bacc-restart-playing.json`. This verifies that saved input/restart case on this Linux runtime; it does not establish all library tracks or the reporting Mac.

The automatic handoffs above still waited for the 20-second WebView watchdog before fallback. These are Linux decoder/clock observations, not physical audibility or macOS validation. Raw MCP evidence is owner-local under `/tmp/burrata-mixtape-*.json`; signed resources are not reproduced here. Latest native tests: 1576 passed/37 ignored. Latest frontend tests: 1161 passed/93 files, zero type-check errors/warnings. Subsequent edits require fresh qualification.

## Earlier stalled-seek transfer

The frontend now attempts handoff after 5 seconds only for eligible whole-video YouTube seeks that have not landed at the target. Ineligible sources retain the original 20-second recovery deadline; loss of eligibility before the early trigger re-arms the remaining original deadline. On the same qualified native binary with this frozen frontend, Rooftop seek to 1800 seconds transferred to FFmpeg and advanced beyond 1836 seconds. Native first audio was 2172 ms after fallback began; exact source and entry `f3d2568f-67a9-44c2-83a5-657c06c4a9f2` were retained. This is one live observation, not a latency distribution or a measured total seek-to-audio time. Evidence: `/tmp/burrata-early-handoff-progress.json`. The final frontend suite passes 1161 tests/93 files and type-checks cleanly (`/tmp/burrata-early-handoff-full-frontend-tests.log`).

## Historical integration evidence

The stage-1 native integration suite passes **1,549 tests, 37 ignored** (`/tmp/burrata-stage1-integration-host-tests.log`). The independent 84-cell matrix runs but exposes four extra-complete-performer mismatches in Balanced/Flexible; those remain under correction. Later shared-model and handoff edits are not covered by this completed run.

The stage-1 executable `.cache/qualification-stage1-20261021/bin/burrata` has SHA-256 `753dacf7106c43b3e61f618d833ee60c064c63596e19a8801a0fb5fd63734b8f`. With the existing native engine selected for this test, the exact Rooftop video `6MAzUT1YhWE` played and sought to 1,800 seconds. Native diagnostics recorded FFmpeg, AAC format 140, **1,939 ms seek-to-first-audio**; position subsequently advanced beyond 2,100 seconds without a failure. This proves local decoder/output progress, not physical audibility or macOS behavior.

The preceding WebView capture reproduced the stalled seek on both the original source and an exact-source refresh forced through the relay. GStreamer qtdemux remained in `fragmented_seek_pending`, emitting early-timeline packets without a target byte-range request. Startup range requests returned 206. A bounded exact-video probe found AAC `initRange` 0–722 and `indexRange` 723–8,122 containing SIDX, both served with exact 206 responses. Therefore a missing provider index is not the explanation. FFmpeg decoded three seconds at 1,800 seconds without a full-file download: AAC first PCM 2,479 ms; Opus/WebM first PCM 6,468 ms. These one-off probes are not latency distributions.

A session-only, same-candidate handoff to Burrata's existing native engine is being integrated for failed whole-track YouTube seeks. Queue occurrence IDs, position and ownership must survive the transfer. It is not yet qualified, and this document does not claim the macOS report is resolved.

## Reported cases

- PROLIFIC / Nipsey Hussle and Bino Rideaux: Vibe With Me rejected by recording verification; Sacrifices briefly played unrelated notes and then silence.
- Bacc of the Bach: works from discography, fails with extraction error from Library after relaunch. The saved/fresh input and cold cache paths need independent comparison.
- Fred again Rooftop Live (Arun's Roof, London): starts, then seek fails with HTTP 403.
- Full Mixtape Playback, Palacio De Los Deportes: starts, then seeking expires after 20 seconds.
- Seek thumb appears to stick to the pointer; loading should remain visible without hover; artist typing should offer suggestions.

## Current evidence

The current official Sacrifices candidate `youtube:vqDoiUMuqQA` is 268 seconds and contains substantial nonsilent PCM in five-second windows at 0, 30 and 134 seconds. This rules out encoded silence in those sampled windows of that candidate; it does not identify the source selected on the reporting Mac or prove the full recording.

A cold isolated saved-track resolver attempt reproduced failure locally. Earlier YouTube and SoundCloud attempts consumed both extraction slots before verified artist aliases were ready. A later candidate was blocked by the extraction budget. The zero-duration helper-ready failure in that trace was a consequence of budget admission, not proof of a failed helper executable. The original wall-clock deadline had not expired. A bounded alias-preparation ordering correction is in progress.

Fresh Discogs release `38549214` still supplies Sacrifices with complete guest credits and no duration/ISRC; missing duration is not unique to the saved fixture. Current normal-library search did not contain the three reported songs before this investigation; catalog read tools are used to obtain fresh inputs rather than claiming the Mac library is available locally.

Frontend changes under qualification add persistent playlist-row buffering visibility, debounced accessible artist suggestions, bounded same-candidate seek refresh and lost-pointer-capture cleanup. They do not yet prove live long-set seeking on Linux or macOS.

Local raw diagnostics: `.cache/playback-v015-mac-20261021/`. No profile or provider credentials are included in this document.

Local GUI reproduction used the app-owned search controls: the exact Fred again Rooftop result `youtube:6MAzUT1YhWE` (6,128 s) played normally, then seeking to 278 s reached the 20-second watchdog without completing. This is a Linux reproduction of the reported symptom, before the new seek-recovery frontend was loaded.

Bacc of the Bach played from its stored Discogs catalog ID through approximately 90 seconds on that earlier runtime; its after-relaunch behavior is still pending. This does not establish that all library entries work.

The native cold-alias preparation correction passed the full suite: **1,513 passed, 37 ignored**, `/tmp/burrata-v015-report-native-tests.log`. It preserves the original deadline and two-extraction cap. The built qualification executable is `.cache/qualification-v015-report-20261021/bin/burrata`, SHA-256 `74fc8a10738eb38ed4656fb2782e8334dd4c672a11888c5ae22b763ee04f68a2`. Live requalification is pending.
