# Helper-free playback search and verified-source access recovery

## Search comparison

Three alternating cold pairs per recording compare the helper-based search with an anonymous native WEB videos-only search. Both use the same extractor, matching/version gates, access checks and five-second PCM decoder. Direct first-choice playback is disabled. No local build runs during either matrix. These are Linux debug/headless observations, not mixer/device or statistical p95 qualification.

| Recording | Legacy median first PCM | Native median first PCM | Result |
| --- | ---: | ---: | --- |
| Matthew / John Denver | 13.906 s | 8.998 s | 3/3 per arm |
| Victory Lap / Pengus | 8.593 s | 7.196 s | 3/3 per arm |

All Pengus requests select `kzrZXghhy1k`. Matthew selects `ps9X-DXUFGY` or `uZGRATckoBM`; two pairs differ in upload ID, so the cohort median is not a matched-upload timing estimate. Both pass existing recording checks; no full-track acoustic equivalence claim is made.

A further one-pair corpus covers Billy Strings, Turtledove, Moiroloi and How Much Are They. The native arm succeeds on all four (8.15–9.56 s), while the legacy Billy attempt fails source access with repeated HTTP 403 despite passing identity checks. How Much uses the independently obtained catalog duration, and its timer excludes the account-backed enrichment step. Every failure is retained.

The final default-build smoke (no runtime search opt-in) succeeds for Matthew (8.97 s), Pengus (8.71 s) and Turtledove (9.35 s), but Billy again fails HTTP 403. That failure motivates the separate transport recovery below; it is not erased by the passing prototype run. The default search has bounded JSON/body/time limits, no cookies/authorization, primary-video-only parsing, cancellation, complete-cache ownership and the original shared deadline. Missing/changed/empty provider layouts fall back to the existing helper; custom helper choices and interactive search retain their previous behavior. `BURRATA_NATIVE_YOUTUBE_SEARCH=0` disables the adapter for rollback or offline unit fixtures.

## HTTP 403 recovery checkpoint

The failing Billy source passes fresh title/ID/duration verification, then both extracted URL formats receive HTTP 403. A public anonymous alternate-profile AAC-140 feasibility probe for the exact same video `qND7vxsWE2Y` produces five seconds PCM. Its timing overlaps a test run and is not application startup qualification. The final default production recovery succeeds in two retained live denied-source attempts: first PCM at 11.800 and 9.821 seconds, with recovery itself taking 1185 and 786 ms. Both use exactly two extraction attempts. A preceding final-pipeline smoke also succeeds. The recovery policy is: use it only after a source passes existing identity gates and returns HTTP 403; require the same video, channel, title and duration from the new player response, validate its URL uncached, and retain the existing absolute deadline and two-extraction budget. AAC preserves the existing seekable MP4 path for WebView. No recording or guest check is relaxed, and first-choice direct playback remains experimental.

## Desktop and source limits

Loaded John Williams recordings now display durations and the same row layout as playlists. The app-only 954×1036 capture was visually inspected and remains private in `/tmp/burrata-loaded-artist-tracks-20261020.png` because its sidebar belongs to the main profile. The trusted middle-click test on the MusicBrainz profile link creates no ChatGPT window. Zero Firefox window delta does not establish whether an existing browser gained a tab, so it does not prove the complete external-navigation behavior or resolve the original report. The library view is restored afterward.

Nighttime in the Backyard still lacks an automatically verified original audio connection. Independent MusicBrainz metadata for its spacing variant identifies recording `0113721b-b226-4357-b69a-5114e4aab7de`, 197760 ms, ISRC BRL6M0900003, but exposes no recording URL relation. No manual mapping or writer-to-performer credit substitution was installed.

The raw JSON and redacted phase streams retain all outcomes. Prototype-reported reused the prototype-paired binary after a worktree policy edit; its manifest explicitly links the compiled-source attribution rather than presenting invocation-time hashes as built code. Default-smoke has its own binary and source hashes. Credentials, account identities, signed media URLs and raw library data are excluded.

## Final default checks and desktop regression

The final native default suite passes 1344 tests, zero failed, 30 ignored, with public search disabled for unit fixtures. Policy and mock HTTP tests independently cover default enablement, literal queries, body/status/layout limits, cancellation, fallback and strict AAC container selection. The complete frontend suite passes 1074 tests before the final pause correction; all 411 App integration tests, including the new attached-relay regression, pass after it. Extension, MCP and player suites pass; Svelte check has zero errors/warnings and the frontend builds. The retained failed CI run at d3c5ddd passes macOS and Windows but fails one Linux fake-browser test because the argument file became visible before its environment append. The fixture now atomically publishes the complete file and stops its child before assertions; no provider runtime change is hidden in that fixture correction.

The disposable WebView test first fails its setup check because a WebView snapshot has no native-only `engine` field; no audio starts. Its corrected inspector-based run plays and actively seeks, then exposes a real automation-pause defect: `nextPreviewRequest` retires the relay URL still attached to paused media, and a seek returns HTTP 404 with a CORS diagnostic. Automation Pause now retains the current attached source generation while pending/stale lookups still cancel. The repeated actual relayed-source test passes playback beyond 3s, active seek beyond 31s, paused seek to 60s and resume beyond 61s, volume zero throughout. The source has two resolver-owned headers and therefore actually uses the relay. This is media-clock/seek evidence, not physical audibility or macOS device qualification. All setup/product failures remain separate artifacts. The main signed-in profile is restored afterward on the new executable; no normal queue replacement was used for the test.

The restored main profile has exactly one GUI on binary SHA-256 `12ef85b55307ebf90a080b9b1c9302cd55cb31c86affaa053a7dba2a0f74b453`; Spotify and YouTube are still confirmed, native transport is ready, and YouTube does not request reconnection. Only the disposable profile received the playback/seek test queue. [Restoration summary](main-restoration.json).
