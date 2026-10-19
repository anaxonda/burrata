# Playback follow-up — 2026-10-18

These are local changes after public v0.1.3. The recordings were tested with exact catalog metadata in fresh temporary stores using the production resolver and five seconds of decoded PCM. This is Linux debug evidence, without a mixer, audio device, GUI or normal-profile mutation. It is not an end-to-end startup deadline or p95 qualification.

| Case | Cause | Current exploratory result |
|---|---|---|
| Sacrifices — Nipsey Hussle/Bino Rideaux/James Fauntleroy II | Verified Discogs artist variation was blocked by application-only provenance overlay fields | Complete credit verified; exact source `vqDoiUMuqQA`, 268s; first PCM 13.92s on final matching code |
| Reckless — Nipsey Hussle/Bino Rideaux/Static Major | YouTube Music parsing retained only the first linked artist | Full performer set retained and independently verified; source `FXIvaktRqZg`, 139s; first PCM 21.41s, so reliable startup remains unresolved |
| Everybody Wants To Be A Cowboy — Kacey Musgraves/Billy Strings | Saved feature suffix versus full source artist prefix | Complete credit/title/duration match; source `qND7vxsWE2Y`, 219s; first PCM 20.01s, leaving inadequate startup margin |
| Matthew — John Denver | Flat search returned candidates 6–7 seconds from the saved 222.426s duration; the exact title-only YouTube Music catalog locator was disabled for known-duration tracks, so weaker candidates consumed both extraction attempts | Added a unique-catalog locator fallback and ranked it ahead of provisional candidates. Playback still requires fresh exact title, complete artist credits, verified channel and integer duration within the existing six-second allowance. Four focused regressions pass; live resolver/decode qualification is pending |

The slow Billy/Reckless traces show a first AAC extraction followed by failed CDN probes and a second full extraction. A request-local alternate-format reuse optimization is implemented and covered by regression tests but not yet live-qualified. It retains the original cancellation/deadline, two-attempt cap, complete identity checks and head/tail probes. No signed URL is persisted as an identity mapping.

France Inter now plays through the actual isolated WebView: media clock advanced from 17.998s to 139.769s with readyState 4, stable source and no media error. The saved HTTP redirect source is resolved and relayed through the validated exact-station transport. This does not qualify the affected physical Mac. Screenshot verification encountered first-run onboarding over the station view; an unobstructed layout/artwork capture remains pending.

Frontend verification after the failure-message follow-up: 1,048 tests pass, Svelte reports zero errors/warnings, production build succeeds. The combined native candidate-ranking/alternate-format batch passed 1,198 tests, with 25 opt-in tests ignored. After the latest matcher change, all four focused offline locator/ranking tests pass. A full native run reports 1,146 passed, 59 failed and 25 ignored; the failures are loopback/network fixtures rejected with `Operation not permitted` when binding local sockets in this sandbox. The live Matthew resolver command was rejected before execution because the automatic approval service returned a model-capacity error; this is a review-service failure, not a test result.

## Failure reporting follow-up

The resolver now distinguishes an empty search from collected results that fail recording identity checks. It retains inaccessible-source and network failures across parallel providers, later providers and alias retries. Previously, a fallback's generic no-match error could replace the useful earlier failure. Deterministic failure selection preserves cancellation/deadline semantics and provider priority for equal categories. A successful fallback still wins.

The frontend and native queue classifiers recognize only the exact fixed new outcomes (`no_audio_results` and `recording_identity_unverified`). Existing outcomes remain compatible. Full fixed reasons survive display sanitization; network failures stay on the current occurrence instead of consuming the queue. The native regressions exercise both provider completion orders, failure preservation, successful fallback and recorded queue categories. Frontend integration tests exercise visible complete reasons and actual queue advancement.

The copied development executable was also missing its expected Linux resource layout, causing helper discovery to fall back to system locations. Subsequent staging uses `PREFIX/bin/burrata` and `PREFIX/lib/Burrata` pointing to reviewed development resources, matching Tauri's resource lookup. This is a development staging correction, not evidence of an installed public-package defect. Runtime helper discovery and the normal-profile relaunch still require verification.

## Blocked verification checkpoint

Latest debug executable SHA-256: `fd18cd5577fdf50b719580a26edf485cffb7b5727a3fcf88a5ec144dbe4e5f5f`. Native build succeeds; 1,048 frontend tests, Svelte checks and frontend build pass. The prior full native suite passed 1,198 tests; the latest sandbox run passed 1,146, with 59 socket-binding failures and 25 ignored. Four focused offline locator/ranking regressions pass.

Pending: live Matthew resolution and PCM decode, live alternate-format timing on Billy/Reckless, loopback-enabled full native suite, final WebView/layout/artwork check, and normal-profile relaunch. The last verified GUI is the muted, paused disposable profile `/tmp/burrata-launch.dc9953a9`, running the earlier `3e66b59852a3` build. Normal-profile queue/account state was captured before testing and was not overwritten. Do not restore an older saved queue snapshot.

Resume with the staged executable recorded in `.cache/playback-bugs-2026-10-18/current-build.json`, retaining exactly one GUI. Before restarting the disposable profile, set only its `releaseSetupComplete.v1` preference to true while its process is stopped, so onboarding does not obscure the visual check. Keep the normal profile's preferences intact.
