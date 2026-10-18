# Magalenha source investigation

The reported catalog item is Simon Fava, Sergio Mendes — Magalenha, Extended Mix (267 seconds), imported from a Traxsource chart without a pinned audio candidate. The reported startup-budget timeout does not establish which search, extraction or validation phase exhausted the budget. Existing post-attempt snapshots contain no resolved source and no correlated phase trace for that attempt.

An earlier packaged-helper flat metadata query failed TLS/connectivity before results. A subsequent single host-permitted query using the same verified packaged yt-dlp returned 20 public YouTube metadata rows in 3,098ms. It used no cookies or account credentials and performed no extraction, playback or provider write. Some explicit Extended Mix rows report 265–268 seconds; other similarly named versions report 292/311 seconds. This demonstrates discoverable metadata under that environment, not a correct playable match or the cause of the earlier failure. Production candidate-gate analysis is pending; identity/version/duration guards remain unchanged.

The separate shared-deadline patch remains staged for review and is not deployed. Private read receipts and the evolving analysis are under `.tmp/playback-source-magalenha-review/`.

## App-mediated metadata check

The running normal app's read-only `search_audio_sources` call for the exact local track returned the 267-second candidate, with all title/artist tokens and the requested version reported as matching. A subsequent single `inspect_audio_source` call returned the same title and 267-second runtime with five formats. Confidence remains `review`; metadata score alone is not a playback eligibility claim. Neither call started playback or downloaded media. Actual stream access, full playback matching and decoding remain unverified. Private app receipts are retained alongside the public flat-search receipt.

The app also returned a strict-eligible clean-credit candidate (rank4 in this app read; rank9 in the separate flat read). Its full metadata inspection retains the exact Extended Mix title, high confidence and 268-second duration, with three formats. The similarly named `feat.` rows are review candidates rather than strict matches under the current exact-credit rule; this distinction is retained in the staged regression. Extraction metadata is available, but stream access and actual decoding remain unverified.

## First isolated WebView attempt — retained failure

One unpinned imported-track attempt ran in a fresh isolated profile on executable `f0ece0d9f984ba41e7f1bc8e547645240dcc91cbe5c9a28fe0c34df97d5ffd84`. The runner submitted one play request, performed no seeks because startup did not qualify, and verified Pause in cleanup. No media progress, physical audibility, acoustic identity or native mixer success is claimed.

Correlated source events show YouTube search returned10 rows at2253ms, with one eligible candidate. Extraction succeeded at7352ms. The initial stream range GET (trace role `head`, not the HTTP HEAD method) failed after8001ms at15421ms, with no response status. SoundCloud search returned20 rows but no eligible candidates. A second extraction began at15426ms before cancellation. This narrows this run to a network/access probe problem after successful metadata extraction; it does not establish HTTP403, expiry, absent media or a specific TCP/TLS cause. Error-category diagnostics are needed.

The harness's SDK request timed out because its startup clock also included preflight reads/muting. It consequently cancelled before observing the app resolver's full20-second deadline. This retained run is not a successful app deadline-bound test, and no automatic retry occurred. The harness timing will be corrected separately. Private report and trace are `.cache/qualification-magalenha-webview/one-shot-private.json` and `phase-trace-private.json`.

The isolated app was closed through its exact owned launcher before the normal app was restored. The full fresh100-occurrence/index2 playback snapshot is exactly equal before and after handoff; no queue was manually reconstructed. The normal app is the sole running instance.

## Second isolated WebView attempt — source resolution succeeds, playback qualification incomplete

The V2 attempt selected `youtube:qySa2Xtq0kU` for the unpinned catalog row `traxsource:14854759`. The sanitized phase trace records a successful search with 10 results, one eligible candidate, successful extraction/validation, and successful head/tail range probes (HTTP 206). The strict full-identity check passed the requested version and provider-ID comparison. It records resolved metadata duration 268 seconds against 267 seconds in the catalog, and the production identity gate accepted that difference. The WebView qualification harness retains a ±1-second media-duration gate.

The resolver finished at 6.641 seconds with `ok=true`, `deadline=false`, and `cancelled=false`. This is direct evidence that V2 completed source search, extraction, stream probing, and the resolver's identity gates. It is not evidence that the WebView sustained decoded playback.

The retained post-attempt WebView snapshot identifies the same YouTube source, with actual media duration 267.633 seconds, source present/stable, no media error, media position 27.124 seconds, and `seekSerial=2`. At capture the app was paused and the audio element had `readyState=2`, below the runner's required 3/4. The single snapshot and missing timeline do not establish five seconds of continuous media-clock progress after start or after either seek. The older V2 runner report has only a global SDK timeout and cleanup status; it omitted its completed start/seek stages. Therefore V2 supports successful source resolution and attachment, but it does not qualify startup progress, seek progress, physical audibility, native mixer consumption, or recording acoustics.

The next candidate-pinned attempt must use a fresh isolated profile, a fresh immutable build/source pin, and the updated stage-checkpoint runner. Pin the expected WebView source as provider `youtube`, recording/source ID `youtube:qySa2Xtq0kU`; keep the imported catalog row unpinned and do not place a candidate in its saved metadata or the play request. Do not reuse the V2 profile or its stale source hash.

Private evidence: `.cache/qualification-magalenha-webview-v2/phase-trace-private.json`, `post-media-private.json`, and `one-shot-private.json`. The normal app was restored with its full playback snapshot unchanged.

## Third isolated WebView attempt — startup progress proven, seek qualification incomplete

One unpinned startup request on candidate `7c8c46e64110` resolved/validated the expected YouTube source in6.820s. Both known-length head/tail range bodies passed the new exact-byte validation. The runner observed ready media at8.970s and then sustained5.349s wall /5.441s media-clock advancement across8 observations with readyState4. Total startup proof completed at14.936s, including the required5-second observation window. This is one WebView progress result, not physical audibility, acoustic recording identity, native mixer consumption, a p95, or a five-second startup-target pass.

The first95% seek did not qualify before the phase deadline. The runner retained startup success and its seek deadline error, but its seek catch still omitted partial target/timeline context. An independent final paused snapshot shows seekSerial1 and media at its end267.633s, readyState4, source present/stable and no media error. It does not establish a source stall or successful bounded seek progress; the harness needs the missing partial observations before attributing the failed qualification. The second seek was not issued. No automatic retry occurred.

Pause cleanup succeeded. The isolated app was closed and the sole normal app restored; full fresh100-occurrence/index2 playback state matches exactly before/after. Private evidence is under `.cache/qualification-magalenha-webview-v3/`, including `one-shot-private.json`, `phase-trace-private.json` and `post-media-private.json`.

## Fourth isolated attempt — target landing and progress captured

The updated runner retained the first seek's partial observations. Startup again passed the five-second owned WebView-clock gate: ready source observed8.725s,5.352s wall /5.387s clock progress across8 observations. Source resolution and validation completed6.796s. These remain single-run cold/debug observations, not a five-second startup-target pass or p95.

The95% seek target253.65s was observed at254.154s at1.277s after the seek stage began, on the same source/session/occurrence with seekSerial1. Media time then advanced monotonically to267.605s at14.735s before ending at267.633s. During this window the browser reported readyState2 and app status buffering, without a media error; after ending it was paused/readyState4. This establishes target landing and media-clock progression, but not future-data readiness, decoded output, physical audibility or the existing five-second ready/playing gate. The qualification therefore still fails; no readiness threshold was relaxed and the second seek was not issued. Event/ready/buffer-range diagnostics are the next step to distinguish buffering-state semantics from decoded-output behavior.

Pause cleanup succeeded. Normal app restoration preserves the exact fresh100-occurrence/index2 playback snapshot. All trial evidence is under `.cache/qualification-magalenha-webview-v4/`. No provider write or automatic retry occurred.


## Fifth isolated attempt — bounded media events narrow the failed seek

Current debug candidate `bad3495b0d82` resolved the unpinned catalog row to the expected YouTube source in6.601s. The ready-source observation was8.105s; startup passed the owned-clock gate with5.224s media advancement over5.138s and eight observations. Startup proof completed14.680s including that observation window. These are single-run debug observations, not p95, decoded output, physical audibility or a five-second startup-target pass.

The first95% seek emitted `seeking` at253.65s with readyState4 and a full buffered range0–267.633s. It then emitted `waiting` and `seeked` with readyState2/networkState2, followed by `stalled` as the clock advanced. The buffered range remained full. The clock advanced to the end, where readiness returned to4 while paused. No media error was reported. This demonstrates the command and target event were processed, but the strict sustained ready/playing seek gate still fails. The second seek was not issued. Buffer ranges and clock movement alone do not establish audible or decoded output, and these events do not identify a CDN or decoder cause. The ordinary direct-stream path has no app media proxy between the element and its selected URL.

Pause cleanup passed, the isolated profile was stopped, and normal-app restoration preserved the complete queue/playback state exactly. Retained evidence: `.cache/qualification-magalenha-webview-v5/one-shot-private.json`, `phase-trace-private.json`, and `post-media-private.json`. Safe selected-format diagnostics and a local codec comparison are the next discriminating checks; source selection and readiness thresholds remain unchanged.
