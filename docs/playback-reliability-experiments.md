# Isolated playback reliability experiments

The experiment is executable but remains outside production. It uses the existing isolated `youtubei.js` **18.1.0** package, the already bundled BgUtils **2.0.2** implementation, FFmpeg, and the existing feature-gated native Opus benchmark. It adds no production dependency, account, GUI, device, paid service, embedded provider player, or mpv path.

## Commands

Run unit and protocol tests:

```sh
node --test scripts/performance/innertube/core.test.mjs scripts/performance/innertube/token.test.mjs
python3 -m unittest discover -s scripts/performance -p playback_reliability_test.py
```

Run the controlled offline matrix: generated 30-second stereo Opus, one immutable byte source served over a loopback-only HTTP Range endpoint, three warmups and 20 measured repetitions for each cold-helper, warm-helper, early, middle and late seek scenario. A corrupt-media failure check runs separately.

```sh
python3 scripts/performance/playback_reliability.py \
  --mode offline --transport loopback --repetitions 20 \
  --output /tmp/burrata-offline-reliability.json
```

For a quick diagnostic, use `--smoke --repetitions 1`. Such a report is explicitly a smoke run. `--transport file` avoids even loopback sockets. `--fixture path/to/recording.opus --duration 300` reuses supplied bytes; duration must describe those bytes. The early/middle/late FFmpeg excerpt positions are 1%, 50%, and 95% of `duration - 5`, leaving five seconds available for duration validation. The native benchmark uses 1%, 50%, and 95% of the full duration and measures the first PCM block.

Compare existing native decoding against retained Opus and fresh FFmpeg using **the same supplied file** or byte-identical loopback stream:

```sh
python3 scripts/performance/playback_reliability.py \
  --mode native --transport loopback \
  --fixture /tmp/same-recording.opus --duration 300 \
  --repetitions 20 --native-budget-seconds 1800 \
  --output /tmp/burrata-native-reliability.json
```

This runs the repository's ignored `seek_decoder_benchmark` twice: default production features (`fresh` / `retained`), then `experimental-opus` (`ffmpegFresh` / `opusRetained`). Cargo uses `--offline`; required crates and system development packages must already be present. It does not open an audio device. The native test has a fixed 20 repetitions per position. Both the baseline and Opus comparison have three warmups per path. Compilation and the complete test process receive an absolute timeout; a failed build or test becomes a typed failure, never a successful empty comparison.

Fault transport modes are `--fixture-fault deny-first`, `truncate`, and `stall`, with `--transport loopback`. Requests record only numeric byte ranges, statuses and counts. They do not log URLs or headers. Failure injection reports failures; it does not turn unexpected failures into passes. The single corrupt-media check declares its expected `DECODE_FAILED` result separately.

Live extraction is opt-in and requires matching explicit recording IDs:

```sh
python3 scripts/performance/playback_reliability.py \
  --mode live --video-id EXPLICIT_ID --expected-video-id EXPLICIT_ID \
  --duration 300 --repetitions 20 --budget-ms 45000 \
  --output /tmp/burrata-live-reliability.json
```

Replace both placeholders with the **same 11-character video ID**. This is a public anonymous network experiment, with three warmups per scenario and 20 measured repetitions, not a search-based recording selection. The harness creates only temporary isolated session directories and removes them after shutdown. It does not touch the normal application profile. `--token-mode none` is an explicit token-free control and reports that mode; it is not automatic token support.

## Prototype lifecycle and invariants

The JSONL worker is `scripts/performance/innertube/worker.mjs`. It requires `BURRATA_BENCH_DATA_DIR` to point to isolated scratch storage. Playback requests must contain `videoId`, an equal `expectedVideoId`, and an integer `budgetMs` in 1–120000. `play` and `seek` decode 5–30 seconds of PCM. Search returns candidates without claiming recording verification.

```json
{"id":"play-1","action":"play","videoId":"abcdefghijk","expectedVideoId":"abcdefghijk","budgetMs":45000}
{"id":"seek-1","action":"seek","videoId":"abcdefghijk","expectedVideoId":"abcdefghijk","position":60,"budgetMs":45000}
{"id":"cancel-1","action":"cancel","targetId":"seek-1","budgetMs":1000}
```

The absolute command deadline starts when the worker receives the request, including queue time. Every awaited stage shares that budget; provider promises that never settle cannot indefinitely block the protocol. Fetches receive the active cancellation signal. FFmpeg and token generation run in owned subprocesses that are terminated on cancellation, with a short forced-kill fallback. The outer harness also enforces a command deadline and kills the entire process group if the helper fails to respond. The worker stops after 30 seconds idle by default (`BURRATA_BENCH_IDLE_MS`), and shuts down its token child at EOF, cancellation, or process termination. Queue depth is bounded to eight commands. The worker has no production import path.

Client, player, and unexpired extraction data persist across commands. Up to eight manifests are retained for at most five minutes, with a 30-second expiry margin. A signed URL's `expire` parameter can shorten that lifetime. Missing expiry information cannot justify caching a manifest. An expired manifest or a stream 403/410 permits **one** refresh inside the original deadline. Provider response recording IDs are checked when present. A matching explicit video ID is a provider identity invariant, not acoustic or fingerprint verification.

Automatic token support uses a separate managed Node process importing the already bundled `SessionManager` from `src-tauri/resources/youtube-support/bgutil/server/build/session_manager.js`. This avoids sharing the production broker, its port, or a normal application session. The child passes the current anonymous Innertube context and binds the token to that context's `visitorData`. It returns the upstream `expiresAt`; tokens refresh 30 seconds early, on binding changes, or on the single denied-stream retry. Tokens are attached to both `session.po_token` (player requests) and `session.player.po_token` (stream deciphering), matching the installed youtubei.js implementation. Upstream token-helper logs are discarded. There are no manually copied token or visitor-data environment variables. Missing bundles or failed token minting produce explicit failures.

This implements token lifecycle mechanics against the shipped bundle. Offline tests use a fake minting subprocess; they do **not** establish that today's public provider accepts the token, client, or decipher evaluator. No live success is claimed by those tests. The minimal VM decipher evaluator remains a possible live compatibility blocker and is intentionally isolated.

## Report and accounting contract

`playback_reliability.schema.json` defines `burrata.playback-reliability/v1`. Every attempt records strategy, scenario, iteration, warmup flag, outcome and available metrics. `firstPcmMs` starts at decoder initialization; live `firstPcmFromCommandMs` also includes worker preparation and extraction from request receipt. Outer `wallMs` includes process startup and completion of the excerpt. These scopes are kept distinct. Measured summaries exclude warmups, retain typed failure counts, and report median, nearest-rank p95 and worst successful latency. Environment/tool versions, source/lockfile hashes, fixture SHA-256, budgets and scope accompany results. A missing native run is a failure, not an absent row silently counted as a pass.

The helper reports typed stages (`session`, `sessionWait`, `token`, `extraction`, `format`, `decipher`, `decode`, `search`) and numeric stage durations. Exceptions, signed stream URLs, visitor bindings and token values never enter failure reports. Failed or cancelled attempts explicitly mark incomplete per-stage accounting.

Successful FFmpeg attempts include main Node CPU, the token child's CPU delta when used, FFmpeg CPU, and the accounting wrapper's CPU. A denied retry's FFmpeg and wrapper CPU are included. CPU for subprocesses is obtained with `wait4`, including descendants they reap. Helper, token helper, FFmpeg and wrapper RSS are reported separately; lifetime high-water marks are not added together or presented as simultaneous resident memory. Complete cold/warm process lifecycle accounting includes initialization, helper shutdown, and reaped descendants, complementing per-command metrics. Native per-attempt fields retain the existing decode-thread and child CPU scopes; whole-run accounting includes its other helper threads and build overhead separately. The Python harness/fixture server's orchestration CPU is outside the playback-helper accounting scope.

Cold means a new helper and isolated session cache. OS page caches are not dropped. Warm-helper rows still launch a new FFmpeg decoder for every excerpt. Only the native `opusRetained` / `retained` rows assert retained decoder behavior. Five seconds of decoded PCM establishes decoding and excerpt length; these CPU-speed probes do not establish five seconds of real-time audible playback.

`runPassed` describes the requested experiment, including expected failure checks. `qualification` remains **false** because these probes do not cover end-to-end audible output, paused seek, rapid supersession, long recordings, reconnects, all failure classes, or all supported platforms. The POSIX accounting path requires Python `wait4`; Linux/macOS mechanics are represented, but Windows support and platform qualification are not claimed. Promotion requires separate application/device qualification and measured comparisons against unchanged production behavior.

## Validation recorded in this change

The offline tests verify bounded never-settling providers, cancellation, exact ID rejection, cache expiry, one denied-stream retry, signed URL refresh, real five-second PCM decoding, redacted failures, token binding/expiry/refresh, missing token assets, token cancellation, idle process shutdown, identical HTTP Range bytes, denial injection, kernel child CPU/RSS accounting, and subprocess timeout. No public provider request is part of the unit tests.

On 2026-10-19, the final Linux offline loopback run passed all 100 measured excerpts (20 per scenario), excluded 15 warmups, and passed the expected corrupt-media failure check. The report validates against the JSON Schema. All 115 successful excerpts include complete helper/FFmpeg accounting. The report retains source hashes from invocation. Later changes add three baseline native warmups and the separate production diagnostic; offline mode does not load that Rust benchmark, and these changes do not alter the recorded offline measurements.

| Scenario | Decoder first PCM median | p95 | Worst |
| --- | ---: | ---: | ---: |
| New isolated helper | 171.62 ms | 196.96 ms | 256.94 ms |
| Warm helper, new FFmpeg decoder | 158.24 ms | 173.93 ms | 241.99 ms |
| Early seek | 189.21 ms | 246.51 ms | 287.28 ms |
| Middle seek | 192.43 ms | 243.57 ms | 260.30 ms |
| Late seek | 172.41 ms | 229.38 ms | 248.93 ms |

Raw attempts, resource accounting, request ranges, environment and hashes are retained in [the offline report](../scripts/performance/innertube/results/2026-10-19-offline-loopback.json). These numbers describe the generated fixture and machine under this run's load. They compare helper lifecycle states within the FFmpeg path; no native-versus-Opus improvement or public-provider compatibility result is inferred. The final unit suite passed **17 Node tests and eight Python tests**. Live extraction and the Cargo native comparison were not run for this report.


## Read-only production resolver diagnostic

`src-tauri/examples/playback_reliability.rs` exercises the production resolver against an explicit public `Track` JSON fixture and public settings JSON in a disposable Store. The headless acquisition constructor shares production behavior. It deliberately does not construct `NativeOutput`, which would open an audio device: a successfully resolved `AudioSource` feeds FFmpeg for five seconds of PCM, with a shared absolute startup budget. Reports label this **resolver plus FFmpeg decoder only**, and remain unqualified for native mixer/device behavior.

The settings file accepts only `resources`, `preferredStreamingSource`, `streamingFallbackEnabled`, `streamingFallbackSources`, `streamingFallbackDelayMs`, `playbackStartupTimeoutSeconds` (20/30/45), and optional tool paths `ytDlpPath`, `ffmpegPath`, `ffprobePath`, `jsRuntimePath`. Before async threads start, this CLI redirects only its child-runtime XDG cache/config/data roots into a temporary directory, preserving HOME and all persistent settings. No normal profile or keyring is read. Before constructing `ConnectionManager`, the disposable Store marks every credential provider session-only and leaves all credential sessions empty; account-backed connections remain disabled. Local assets, playback ranges and preview URLs are rejected for this public catalog diagnostic.

Example public settings:

```json
{
  "resources": "/absolute/path/to/burrata/src-tauri/resources",
  "preferredStreamingSource": "monochrome",
  "streamingFallbackEnabled": true,
  "streamingFallbackSources": ["youtube", "soundcloud"],
  "playbackStartupTimeoutSeconds": 20
}
```

One explicit public-network reproduction:

```sh
cargo run --manifest-path src-tauri/Cargo.toml --example playback_reliability -- \
  --track .cache/playback-bugs-2026-10-18/matthew-track-fixture.json \
  --settings /tmp/public-playback-settings.json \
  --output /tmp/matthew-production-resolver.json \
  --scenario cold --smoke --repetitions 1 --warmups 0
```

Use `--scenario both --repetitions 20 --warmups 3` for cold and warm resolver matrices. Cold resets the acquisition/store context and stops the owned BgUtils helper before each attempt; OS and packaged-tool caches remain. Warm retains the isolated resolver context. `--position SECONDS` probes a five-second excerpt after a requested seek. `--expected-video-id ID` optionally enforces a specific YouTube recording; nonmatching resolved upload identities are retained in `unexpectedSourceIds` with `UNEXPECTED_SOURCE_ID`, even when PCM decoding succeeds. An upload ID mismatch alone does not establish a different recording. Every resolved candidate's allowlisted public identity remains in the attempt for review; signed URLs, headers, raw provider errors and metadata payloads are excluded. The report checkpoints after each attempt and exposes cancellation, typed failures, durations, and CPU/RSS scopes. Its schema is `burrata.production-resolver-diagnostic/v1`, separate from the extraction experiment report.

Default invocation is 20 measured warm attempts after three warmups. Fewer repetitions or warmups require `--smoke`. The example contains five offline protocol/identity tests (`cargo test --example playback_reliability`). Its initial five tests and runnable dev build passed; the single authorized live execution is recorded below, separately from offline fixture results.

The first authorized live production smoke ran on 2026-10-19 with YouTube primary, fallback disabled, the configured 20-second budget, zero warmups, and expected video ID `uZGRATckoBM` from the previously recorded public official-channel search. A checked-in copy of the exact public input is [Matthew's catalog fixture](../scripts/performance/innertube/fixtures/matthew-public-track.json); the report records input hashes and [the public settings used](../scripts/performance/innertube/results/2026-10-19-matthew-production-settings.json). Resource paths can be overridden with `--resources` on another checkout.

Production resolution succeeded in **11,037.31 ms**; FFmpeg produced **240,000 frames / 5,000 ms PCM**, with first PCM **12,097.22 ms** after the request. The resolved candidate was `youtube:ps9X-DXUFGY`, public title `JOHN DENVER - MATTHEW.`, duration 225 seconds, confidence `review`, official `false`. This differs from the selected expected video, so the original report retains an explicit-ID gate failure in its legacy `wrongMatches` field and `runPassed` remains **false**. It does not establish that two different uploads contain different recordings. The raw report predates the subsequent helper fix normalizing the `youtube:` ID prefix; the observed `ps9X-DXUFGY` still differs from `uZGRATckoBM` under that corrected comparison. Prefix normalization is covered by the offline identity unit test.

[The raw live report](../scripts/performance/innertube/results/2026-10-19-matthew-production-smoke.json) and [redacted production phase log](../scripts/performance/innertube/results/2026-10-19-matthew-production-smoke.log) retain the outcome. The helper shut down successfully. Whole-run helper CPU was 83.31 ms; reaped extractor/BgUtils/FFmpeg child CPU was 7,470.24 ms. Helper lifetime maxRSS was 61,988,864 bytes and reaped-child maxRSS was 349,143,040 bytes; these maxima are not additive simultaneous memory. No 20-run live matrix or native mixer/device qualification is inferred from this single smoke.


## Matthew upload identity and excerpt investigation

A separate [ordinary catalog smoke](../scripts/performance/innertube/results/2026-10-19-matthew-ordinary-catalog-smoke.json), without an expected-upload override, resolved the same `ps9X-DXUFGY` candidate and produced all five seconds of PCM. Resolution took 13,617.45 ms; first PCM arrived 13,922.66 ms after the request. This is one exploratory success and supplies no p95 estimate. Both catalog smokes used the previously built diagnostic binary, whose compiled example-source hash is retained in each report. Subsequent example changes normalize prefixed IDs and name future exact-ID failures `UNEXPECTED_SOURCE_ID` / `unexpectedSourceIds`; they do not revise either original outcome.

The unchanged public official reference `uZGRATckoBM` reports title/track `Matthew`, artist `John Denver`, album `Back Home Again`, release year 1974, a verified channel, and duration 228 seconds. The selected upload reports title `JOHN DENVER - MATTHEW.`, uploader `Electricchris1`, duration 225 seconds, and no structured track/artist/album credits. The requested Spotify fixture is the 222.426-second compilation entry on `The Essential John Denver`. These public metadata fields are consistent with a studio-source hypothesis; they do not establish complete edit/master equivalence.

The isolated `playback_reliability_acoustic.py` uses the bundled yt-dlp mweb/progressive selector and bundled BgUtils in a private loopback process, with at most one same-video alternate WebM retry inside each video's absolute budget. It preserves explicit IDs, allowlists public metadata, and removes raw extraction metadata and PCM when the analysis ends. Existing bundled `fpcalc` produces Chromaprint vectors; optional already installed NumPy supports temporal correlation. No production dependency changes are needed.

```sh
python3 scripts/performance/playback_reliability_acoustic.py \
  --resources src-tauri/resources \
  --left-video ps9X-DXUFGY --right-video uZGRATckoBM \
  --seconds 30 --budget-seconds 20 \
  --output /tmp/matthew-excerpt-equivalence.json
```

One [completed 30-second comparison](../scripts/performance/innertube/results/2026-10-19-matthew-excerpt-equivalence-no-rate-search.json) found Chromaprint agreement **0.9233** over **218 aligned frames** (535 differing bits of 6,976; shift three fingerprint frames). Direct normalized PCM correlation had absolute value **0.1450**, with a 0.3916-second shift. The fingerprint correspondence supports related or matching opening musical content; direct sample alignment and complete-recording equivalence remain unproven. The numeric evidence is retained, without promoting this heuristic to a verified-recording claim.

The reference's AAC format140 returned 403; its same-ID Opus251 retry enabled that completed comparison. A later rate-drift investigation and [the final bounded check](../scripts/performance/innertube/results/2026-10-19-matthew-excerpt-equivalence-final.json) were denied during PCM capture, and every owned helper stopped. Earlier failed attempts remain in the results directory. The rate-search implementation passes a synthetic known-drift test; no live rate-adjusted result is claimed. Work stopped after the agreed final check. No 20-run live matrix, live p95, proven wrong-recording result, or complete-version qualification is claimed.

Final experiment validation is **17 Node tests, eight Python tests, and five Rust example protocol tests passing**. The Rust example test build and runnable dev build passed; later protocol fixes also passed the five example tests. Code is frozen pending root verification; no additional Cargo or public-network work is queued.

## Foreground helper prewarm experiment

The 13.923-second ordinary Matthew smoke spent 6.045 seconds searching, 5.665 seconds extracting (including 2.303 seconds waiting for the bundled BgUtils helper), and about 1.884 seconds on stream probes after initial 403 responses. The earlier smoke's helper wait was 2.568 seconds. This suggested overlapping local helper loading with actual foreground playback search. Standalone flat searches must continue to avoid helper initialization.

The diagnostic now has an **opt-in** `--prewarm-provider` flag. It starts only the locally owned provider helper alongside the production resolver, shares the original absolute deadline, aborts incomplete warmup work when foreground work finishes, and preserves final helper shutdown. Token minting still occurs only when extraction asks for it. The default remains unchanged. This is an isolated diagnostic experiment, not a production implementation.

The bounded A/B runner uses the same binary and public inputs, alternates AB/BA/AB order, starts a new disposable context/provider per attempt, and retains failure rows. It excludes source-ID mismatches from paired latency deltas. This initial diagnostic uses three observations per path, zero additional warmups, and explicitly reports no p95 or qualification.

```sh
python3 scripts/performance/playback_reliability_prewarm_ab.py \
  --binary .cache/verification-target/debug/examples/playback_reliability \
  --track scripts/performance/innertube/fixtures/matthew-public-track.json \
  --settings scripts/performance/innertube/results/2026-10-19-matthew-production-settings.json \
  --pairs 3 --output-directory /tmp/matthew-prewarm-ab
```

[The initial comparison and raw attempts](../scripts/performance/innertube/results/2026-10-19-matthew-prewarm-ab/comparison.json) recorded six successful resolver/five-second-PCM attempts and six successful helper shutdowns:

| Pair | Path | Resolved video | First PCM | Helper readiness wait during extraction |
| --- | --- | --- | ---: | ---: |
| 1 | Baseline | `uZGRATckoBM` | 16,192.35 ms | 3,057 ms |
| 1 | Prewarm | `ps9X-DXUFGY` | 13,482.56 ms | 2 ms |
| 2 | Prewarm | `ps9X-DXUFGY` | 16,536.54 ms | 4 ms |
| 2 | Baseline | `ps9X-DXUFGY` | 12,716.92 ms | 3,046 ms |
| 3 | Baseline | `ps9X-DXUFGY` | 15,863.88 ms | 3,040 ms |
| 3 | Prewarm | `ps9X-DXUFGY` | 13,592.76 ms | 2 ms |

Prewarm removed the observed readiness wait from extraction (median 3,046 ms versus 2 ms), but these observations do **not** establish lower total startup latency. Pair 1 selected different upload IDs and is excluded. In matched pair 2 prewarm was 3,819.62 ms slower; in pair 3 it was 2,271.12 ms faster. The matched mean/median was therefore **774.25 ms slower** with prewarm. Search medians differed substantially (6,914 ms baseline versus 3,249 ms prewarm), while extraction medians were 7,957 ms versus 9,437 ms. Format, CDN edge and network timing were not fixed. Comparing unmatched path medians would overstate the evidence.

No production promotion is recommended from this small sample. If a later controlled comparison establishes benefit, the precise production insertion point is the foreground YouTube provider branch in `playback_sources` immediately before candidate search, not `AcquisitionManager`'s shared flat-search entry points or argument builder. It must acquire an owned provider-use lease, share the foreground deadline/cancellation signal, and release/abort on no candidate, cancellation, failure or supersession. The lease must keep the idle reaper from killing a helper still needed by a concurrent invocation and must not kill another active invocation's helper. Existing `provider_invocation`/`ProviderInvocation` ownership can underpin that lifecycle; a detached `tokio::spawn(ensure_provider())` without such ownership is insufficient for production. Identity/version gates, candidate ordering and extraction attempt limits should stay unchanged.

The opt-in path and report parser are verified by the five Rust example tests and nine Python tests; all pass. No production acquisition/resolver source was changed for this experiment. The runnable example build passed before the six live observations, and its exact binary SHA-256 is retained in the comparison report. No additional network or Cargo work is queued by this experiment.
