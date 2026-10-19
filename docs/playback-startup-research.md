# Playback startup research

Research date: 2026-10-19. This is an implementation and experiment proposal, not a performance qualification or a production change. It uses public, anonymous provider requests, the existing native decoder, and disposable profiles. It excludes mpv, embedded provider players, paid infrastructure and normal-profile credentials.

## Recommendation and measured starting point

Prioritize **starting full verification from an eligible search batch while remaining searches continue**, then **reusing extractor process/state**. Both can remove serial work without relaxing recording checks. Expiry-aware URLs, format failure handling and next-track preparation already exist; their experiments should improve the existing implementation rather than replace it.

The [existing measurements](playback-reliability-experiments.md) locate the current cost. Matthew's first public smoke took 11,037 ms to resolve and 12,097 ms to first decoder PCM: search 3,617 ms; extraction 6,974 ms, including provider readiness 2,568 ms; access probe 341 ms. Another ordinary catalog smoke reached first PCM at 13,923 ms, including search 6,045 ms and a failed access probe followed by a retry. These are individual Linux observations, not audible-output measurements or latency percentiles.

The [three-pair provider-prewarm A/B](../scripts/performance/innertube/results/2026-10-19-matthew-prewarm-ab/comparison.json) removed readiness wait (path medians 3,046 ms to 2 ms) but did **not** establish faster end-to-end startup. One pair selected different uploads and was excluded; the remaining paired differences were 3,820 ms slower and 2,271 ms faster, averaging **774 ms slower** with prewarm. No production prewarm promotion follows from this sample. Different upload IDs alone also do not prove different recordings; the original expected-ID failure remains preserved.

## Inspection baseline

Local inspection ended at repository HEAD `fe2a8ee0ea82383d9965b3655a144270bdcc0e84`; the shared working tree may contain concurrent changes. Relevant inspected file SHA-256 values are:

| File | SHA-256 |
| --- | --- |
| `src-tauri/src/acquisition.rs` | `d51cc69f6b1ac816c9cd978f2bafa49c629449daddeb458e4a23a47cdc8dabcc` |
| `src-tauri/src/playback_sources.rs` | `837f45d886522d70e179d2c9cecf056cab9143cafe4a185f741c6c8aaa65796c` |
| `src-tauri/src/playback.rs` | `523c833b75c88b73bf5df7295c50d70e2f745056fa2a175831e26e6755d02c27` |
| `src-tauri/src/native_output.rs` | `f8b032ad947dba1b16adfda98cf561af099d181d76cf5f0313dc7c4a32181db2` |
| `src-tauri/src/youtube_support.rs` | `28784dbf927003944d54c76d754d4571a6f595dcb6d8a63fc0a717a1dd03af9d` |

The Linux tool lock pins yt-dlp `2026.10.10.233141`, upstream source commit `8973412` (the lock records this abbreviated revision), executable SHA-256 `60606cef1a65d70f6639a85b45ebb7acf08dbb81831e134dd93939e23ca2b792`, Node `24.21.0`, and FFmpeg `8.1.3`. The isolated Python experiment's matching release archive resolves the full revision to `897341241f2bfb2439fe0367f4c7c9ccd29a3828`, archive SHA-256 `f983c6d3ded7148de5b8b22a7db41e12cef05f38f1b5396985d969593efad059`. BgUtils provider is `2.0.2`, revision `26475e9d3665b972c4aeb83b4dbaafcf64b88438`. The isolated Node prototype uses youtubei.js `18.1.0` and its checked-in npm integrity lock. Sources below use those revisions/tags where available; current upstream documentation is explicitly separate from shipped behavior.

## 1. Verify eligible candidates before every search branch finishes

### Existing barriers and protections

[`find_candidates_with_cancel_until_alias`](../src-tauri/src/acquisition.rs) runs providers with `buffered(2)` and collects the full result. Within each provider, `search_provider_candidates` runs up to two queries sequentially. The yt-dlp flat query uses `--dump-single-json`, so no candidate is published until that command returns. The playback cache then waits for `complete_playback_candidate_search`, which can add a YouTube Music catalog locator search for up to three seconds while preserving eight seconds for extraction. Foreground and preload searches intentionally populate the same **complete** cache result; a flat-only entry must not suppress later catalog enrichment.

[`resolve_serial`](../src-tauri/src/playback_sources.rs) evaluates recording gates, sorts by evidence priority, and only then caps attempts. Canonical recording, verified alias, narrowly eligible Topic, catalog locator and provisional paths differ in strength. Fresh extracted identity, version and duration checks remain necessary before activation. `SourceTrace::try_spend_extraction` already shares a maximum of two fresh yt-dlp extractions across branches and retries; cached metadata reuse does not spend that counter.

### Proposed first experiment

Add a playback-only bounded batch interface that publishes each completed provider/query batch and a separate final-complete result. Keep the existing complete cache contract. Initially reuse the current whole-command JSON parser: overlap extraction of a strong first batch with the second query/catalog enrichment. This tests overlap without simultaneously changing subprocess framing.

The scheduler should:

1. Apply the current gate evaluator and evidence ordering to each batch; start at most one speculative extraction for an eligible canonical strong candidate. Weak provisionals continue to wait for the complete ranking.
2. Keep search running under the original absolute deadline and request-wide extraction counter. Do not initialize BgUtils for flat search itself. Only actual foreground extraction acquires the managed provider lease.
3. Re-rank when another batch arrives. Before consuming a second extraction, prefer the strongest known untried candidate. Use an explicit ordering frontier: a fully verified result can commit only when unfinished branches cannot outrank it under the chosen policy. If that cannot be demonstrated, hold the result until those branches complete; measure overlap savings separately from any proposed selection-policy change.
4. Preserve provider preference/grace and all pinned-source behavior. An explicitly selected source must retain its explicit provider/upload identity. Success cancels owned remaining work; cancellation, expiry or generation change discards speculative results and prevents mappings/cache publication.

Do not make a first-response race decide recording identity. A later official or exact-version result may outrank an early plausible upload. If equal-priority tie ordering cannot be preserved, record this as a changed selection policy and evaluate it independently.

A later experiment can publish individual flat entries using JSONL. Upstream distinguishes per-video `--dump-json` from whole-playlist `--dump-single-json`; `--lazy-playlist` processes entries as received. This makes streaming plausible, but does not prove that a specific search extractor emits its first eligible row earlier. Validate the bundled command with captured fixtures and timestamped stdout before relying on it. Enforce line size, total bytes, candidate count and backpressure limits; stderr remains redacted. [Upstream option definitions](https://raw.githubusercontent.com/yt-dlp/yt-dlp/master/yt_dlp/options.py).

### Falsifiable acceptance

Use delayed offline batches: early exact match; early wrong version rejected after extraction; later stronger candidate arriving before extraction starts; first batch empty; malformed/oversized JSONL; search failing after a valid batch; two requests sharing a cache flight; cancelled waiter; deadline during extraction; supersession before commit. Assert unchanged gate decisions, no incomplete cache marked complete, no third fresh extraction, one terminal event, and no post-cancel mapping or activation.

For public testing, compare current versus overlap on the same predeclared catalog fixtures and provider settings, alternating order. Record final upload IDs and evidence. Count changed identities, rejected versions and failures alongside latency; do not discard them. Also run fixed explicit-video tests to separate extractor noise from search selection. Start with three bounded exploratory pairs; only proceed to 20 measured repetitions after three warmups per path if identity behavior is acceptable.

## 2. Reuse extractor process and compatible state

Upstream supports Python embedding and structured output, including `extract_info(..., download=False)` and `sanitize_info`; the return value is not guaranteed to be directly JSON serializable. The documentation does not establish a cancellable, thread-safe daemon contract. [Embedding documentation](https://github.com/yt-dlp/yt-dlp#embedding-yt-dlp).

At shipped source revision, `YoutubeDL` retains instantiated extractors in `_ies_instances`, lazily retains a request director, and closes network resources through `close()`. This supports testing object reuse; it does not justify concurrent mutation of one object. [Pinned YoutubeDL implementation](https://raw.githubusercontent.com/yt-dlp/yt-dlp/8973412/yt_dlp/YoutubeDL.py). YouTube's extractor separately caches downloaded player code and processed player data keyed by player identity; some cache paths are memory-only. A persistent worker could therefore save more than interpreter startup, but benefit depends on player/client reuse and the JS solver. [Pinned YouTube extractor](https://raw.githubusercontent.com/yt-dlp/yt-dlp/8973412/yt_dlp/extractor/youtube/_video.py).

### Controlled comparison

| Arm | State retained | Purpose |
| --- | --- | --- |
| A: current bundled CLI | Existing application caches and managed helper policy | Production control |
| B: persistent interpreter, fresh `YoutubeDL` per request | Imports/runtime only; close each object | Isolate process/import cost |
| C: serialized persistent `YoutubeDL` | Extractor/player/network state within one anonymous session epoch | Test additional reuse |
| D: existing youtubei.js experiment | Node session/player caches and separate token child | Compatibility research, not a replacement control |

Keep identical explicit video ID, format selector, managed mweb client, headers policy and provider preparation state for A/B/C. Keep helper-cold and helper-warm runs separate. Reject an arm's identity mismatch as a failure. First compare extraction-only, then unchanged native decode to first PCM; measuring only `getInfo` hides decipher, token and access costs. Report process start, session initialization, helper readiness, extraction, format selection, decipher, access validation, first PCM, completion, aggregate CPU and peak RSS for every owned descendant. Compare at least 20 measured repetitions after three warmups per path, including same-ID repeat and different-ID/same-player sequences. Do not turn successful warm results into a cold-start claim.

The shipped Linux executable is a standalone frozen tool, not an importable Python package supplied to an existing Python worker. Arms B/C require a pinned packaged Python/module environment or a custom frozen worker entrypoint, plus release/licensing review across targets. No runtime dependency or production packaging change is proposed by this document. A resident subprocess that merely shells out to the same CLI for each request does not reuse its extractor state.

### Worker ownership and invalidation

Use a bounded request queue and one active extraction per worker; include a protocol version, request ID, session epoch and absolute remaining budget. Queue time consumes that budget. Avoid modifying shared extractor options mid-request: profile/client/runtime/provider changes select a different worker or retire the old one. Idle timeout, request-count and RSS limits retire workers; crash, malformed response and timeout restart them without accepting late responses.

For hard cancellation, terminate the owned worker process group and rebuild on the next request; do not inject asynchronous exceptions into Python threads. Provider use needs a lease so cancellation cannot kill another caller's active shared helper. Account switch/logout, settings/tool revision change and explicit invalidation advance the epoch and invalidate pending results, URLs and session caches. For anonymous experiments disable browser cookies, netrc and user config, use an isolated writable cache, and delete it on shutdown. Never serialize raw info dictionaries, signed URLs, tokens, cookies or headers into reports. Upstream loggers can expose these values; use allowlisted typed stage events instead.

### Token reuse is a separate compatibility gate

The current [PO Token Guide](https://github.com/yt-dlp/yt-dlp/wiki/PO-Token-Guide) distinguishes GVS, player and subtitle contexts; binding can depend on video, visitor/account session, client and platform. It discourages manual extraction because video-bound tokens require per-video handling. Token lifetimes vary; a token expiry is not a signed media URL expiry. Do not hard-code a universal reuse lifetime or copy a token between arbitrary clients/videos. This proposal uses the existing anonymous provider rather than a provider player or account login.

The pinned [BgUtils HTTP plugin](https://raw.githubusercontent.com/Brainicism/bgutil-ytdlp-pot-provider/26475e9d3665b972c4aeb83b4dbaafcf64b88438/plugin/yt_dlp_plugins/extractor/getpot_bgutil_http.py) forwards yt-dlp's `get_webpo_content_binding(request)`, challenge, network settings and InnerTube context. Its [session manager](https://raw.githubusercontent.com/Brainicism/bgutil-ytdlp-pot-provider/26475e9d3665b972c4aeb83b4dbaafcf64b88438/server/src/session_manager.ts) retains token/minter caches and exposes invalidation. Its six-hour default is implementation policy, not a provider guarantee; internal retry delays also require an outer hard deadline.

At initial inspection, experimental `token.mjs`/`token_child.mjs` always bound to `visitorData`, which was not demonstrated equivalent to production's context-dependent binding. The later corrected-binding checkpoint below separates player/GVS contexts and demonstrates one decoded public source. Broader request-context/video binding, bypass-cache refresh and client compatibility still need qualification. Existing offline tests establish local protocol behavior. youtubei.js v18.1.0 supports session caching with version checks and player caching keyed by player ID/library version, so reusable state is feasible within its API. [Session source](https://raw.githubusercontent.com/LuanRT/YouTube.js/v18.1.0/src/core/Session.ts), [Player source](https://raw.githubusercontent.com/LuanRT/YouTube.js/v18.1.0/src/core/Player.ts). These facilities do not establish parity with the bundled yt-dlp clients or token providers.

## 3. Monochrome versus YouTube, without changing preferences silently

The user's faster Monochrome observation is plausible from the code: its direct `Client::inspect` branch returns before yt-dlp setup, managed YouTube token preparation and the extraction counter. It still needs its own recording proof and stream validation. The Monochrome branch does **not** run YouTube extraction; a separate YouTube branch in a fallback race may already have started its own extraction. No matched provider timing matrix has been collected for this research, so the speed difference is currently an observation and architectural hypothesis.

`resolve_inner_prepared` already starts the preferred provider and first enabled fallback concurrently. `prefer_with_grace` polls both immediately, holding an early fallback success until `streaming_fallback_delay_ms` expires or the preferred attempt fails. This is a preferred-source grace, not a Monochrome-only head start. Additional fallbacks run after failure; pinned sources and one-attempt paths preserve their serial policy. Existing verified provider mappings can bypass rediscovery but still revalidate the source.

Measure four explicit disposable settings: YouTube only; Monochrome only; YouTube preferred/Monochrome fallback; Monochrome preferred/YouTube fallback. Preserve and report the grace setting. Use catalog recordings with strong artist/title/version/duration proof and verified cross-provider correspondence; upload IDs cannot be equated across providers. Alternate provider/order runs, three warmups then 20 measured requests per path. Report match rate, no-match rate, first verified source, first decoder PCM, source/provider chosen, helper/extractor launches, CPU/RSS and cancellation latency. Repeat with an already verified mapping to quantify lookup savings separately. Keep Matthew's uncertain complete-version equivalence outside a matched-recording success claim.

A possible later treatment is a short, bounded Monochrome lookup before starting expensive YouTube extraction, **only** for unpinned requests with fallback enabled and under an explicit tested policy. Compare against the existing immediate race: delaying YouTube can harm misses, while current racing costs extra work. Preserve user primary preference and grace; do not silently change settings or accept weaker identity. A negative lookup cache, if added, must be short, bounded and keyed by recording/version/provider/profile, and contain only definitive no-match results. Never cache 429, timeout, transport failure, cancellation or auth/session failure as a no-match; service backoff remains separate.

## 4. Existing optimizations and remaining experiments

| Area | Already implemented | Narrow next experiment |
| --- | --- | --- |
| Expiring media URLs | `preview_audio_cache_ttl_at` takes signed expiry minus 30 seconds; ambiguous/malformed signing fields get zero TTL. Exact URL access cache/cooldown, session stamps, in-flight invalidation and fresh same-candidate retry exist. | Boundary fixtures for expiry during queue wait/seek; verify one fresh extraction and no stale session republish. Do not extend TTL merely because a token is fresh. |
| Format failures | Bounded profile/format cooldown distinguishes retryable unsupported audio/configured access failures from transient network/cancel/identity failures. Freshly extracted URLs of the same format remain eligible after an old signed URL is denied. | Same-video AAC denial then Opus success and the reverse; record cooldown hits and alternative success. No global format blacklist from one CDN failure. |
| Validation and reader | `validate_prepared_preview_audio` checks DNS and bounded byte ranges, including parallel start/tail for known large lengths. Native decode then opens a separate `StreamDownload` reader. | The completed test-only handoff experiment retained three requests, filled a large gap after tail seek/rewind, and worsened first PCM. Do not promote that prototype; any alternative must avoid the gap fill and preserve response-level validation. |
| Next queue item | `prepare_next` prefetches stable candidates early, waits until at most 30 seconds remain before resolving expiring URLs/decoder, and checks queue generation/cancellation. | Compare existing 30-second window with 45 seconds on unchanged queue fixtures. Measure inter-track gap, success, abandoned preloads, URL expiry, CPU/RSS and network requests. |

Reader integration must reject truncated or oversized 206 bodies, inconsistent Content-Range/length/MIME, identity mismatch and stale session epochs before activation. Seeking must preserve range support and cancellation while headers are pending. One reader may retain validated bytes; it must not turn a full GET or decoder allocation into an unbounded preflight. Queue replacement, repeat/reorder, pause, supersession and account reset must prevent stale activation and release resources. Validate cancellation of a joined cache waiter without destroying another request's valid flight.


### Measured reader handoff result

The [native loopback experiment](performance-results/2026-10-19-validation-handoff/README.md) compared current parallel head/tail preflight followed by a fresh native reader against validation and decode through one owned `StreamDownload`. It used three paired trials at each of 0, 40 and 100 ms simulated response delay, with exactly equal 12,000-sample PCM excerpts. Median first PCM was **50.34 versus 424.57 ms**, **139.16 versus 544.54 ms**, and **245.61 versus 720.89 ms**, respectively, for preflight versus handoff.

The handoff removed the second request beginning at byte zero, but tail seek and rewind caused a large gap fill; both paths still made three requests. Median successful server body writes were 59,392–83,968 bytes for preflight and 1,456,428–1,849,644 bytes for handoff. These are server write counts at arm completion, not client-received or first-PCM network bytes. The naive same-reader approach should not be promoted from this evidence.

An [unguarded first failure](performance-results/2026-10-19-validation-handoff/first-failure.json) also showed that a completed reader operation can hide truncated-response repair: the downloader fetched follow-up ranges and produced PCM where preflight rejected the bounded response. The rerun preserves that control and uses a test-only response-error observer to reject wrong ranges, truncated tails and corrupt audio before PCM in both arms. All six safeguards passed. This is baseline equivalence research, not a proposal to reject all recoverable network retries. Production reader and validation behavior remain unchanged; a different prefix-buffer design would need its own evidence.

## Reproducible reporting and promotion boundary

Extend the existing machine-readable experiment reports with `experiment`, `arm`, `pairId`, `iteration`, `warmup`, input/tool/source hashes, session policy, absolute budget, provider preference/grace, candidate batch timestamps, gate class, selected public identity, extraction attempts, cache state, expiry margin, first PCM, typed outcome and descendant accounting. Keep `qualification: false` until native/device application scenarios and supported targets are independently exercised. Failures remain rows; summaries exclude warmups and show successful sample count plus failure denominators. A first-PCM diagnostic is decoder-only unless a real mixer/device boundary is measured.

First deliver the batch scheduler's offline delayed-result tests and an isolated worker protocol fixture. Run the bounded exploratory public comparisons only after those pass. Promote neither overlap nor worker reuse solely on a helper readiness reduction: require unchanged recording/version behavior, no additional deadline or cancellation failures, clean helper shutdown, and repeatable end-to-end improvement on a same-input matrix. This research adds no production runtime code, dependencies, profile changes or new live qualification results.

## Initial executable experiments

### Flat-search visibility

[`early_candidate_experiment.py`](../scripts/performance/early_candidate_experiment.py) performs public flat metadata searches without stream extraction, plugins, JavaScript runtimes, user configuration or cookies. It caps stdout/line/stderr sizes and request time, drains both pipes, alternates JSONL/aggregate order, and checkpoints per-attempt rows. The [first restricted-network matrix](../scripts/performance/results/early_candidate_experiment.json) retained 12 failed requests with zero candidates. The separate [public-network matrix](../scripts/performance/results/early_candidate_experiment_escalated.json) completed all 12 requests: two fixed queries, three pairs each, 20 seconds maximum per request, ten results per query. It records the bundled binary and script hashes.

Both public arms explicitly used `--lazy-playlist`; this isolates JSONL versus aggregate visibility with lazy processing, and is **not** an exact comparison against production's current non-lazy aggregate command. JSONL's median first-to-last-row interval was **22 ms**. Across the six alternating pairs, JSONL first-row availability was 186, -389, 74, 93, -1,508 and 82 ms relative to aggregate, a paired median **78 ms slower**; negative values mean earlier JSONL arrival. Network variation dominates these few observations. The selected search result sets also changed between paired requests. No warmups, p95 or playback qualification is claimed.

In two JSONL runs the `ps9X-DXUFGY` row arrived roughly 127–140 ms before process completion; that gap includes process teardown and is not demonstrated savings from starting playback. Its 225-second flat duration differs from the 222.426-second catalog fixture by 2.574 seconds, outside the canonical two-second duration tolerance. The report's initial `strong_flat_hint` label is a diagnostic text/duration heuristic, not the production gate. Bare-title hints similarly do not prove artist identity. No row in this experiment underwent fresh extractor, recording/version or stream-access verification. Use the revised harness's explicit hint labels for new reports and preserve the original captured report rather than relabeling its bytes.

The separate [offline reclassification](../scripts/performance/results/early_candidate_experiment_analysis.json) excludes bare-title rows from title/duration hints and retains the original capture hash. Its five-second and seven-second thresholds are explicitly heuristic, not gate parity. The [overlap simulation](../scripts/performance/results/early_candidate_overlap_simulation.json) illustrates scheduling with invented timings; its provisional speculation before the full priority frontier is a divergent toy policy. Its apparent savings are not measured extraction results, cancellation verification or evidence for production promotion.

These results support testing overlap between completed query/catalog batches before adding JSONL framing complexity. The proposed safe scheduler still needs its own execution test; the public visibility experiment has not implemented or measured production scheduling.

### Pinned extractor reuse feasibility

[`extractor_reuse_experiment.py`](../scripts/performance/extractor_reuse_experiment.py) and its [nine-attempt matrix](performance-results/2026-10-19-extractor-reuse/result.json) compare the bundled CLI, a fresh Python process/object, and one serialized resident Python object. A [preceding CLI smoke](performance-results/2026-10-19-extractor-reuse/smoke.json) successfully decoded five seconds. Public phases were serialized after the flat-search experiment. The Python source version/full revision and archive hash match the tool lock; this does not establish that its Python 3.14.8 environment has all dependencies or frozen-build behavior of the bundled executable.

The matrix used three fixed generic public diagnostic IDs, **not Matthew or matched music recordings**, and zero warmups. Each arm received a maximum 20-second request budget. Resident startup was lazy; its first request included readiness within that same horizon. A separate shared warm BgUtils helper took 1,381 ms to become ready before the arms; that setup cost is excluded from arm timings. The resident worker's observed readiness wall time was 320 ms. Per-video arm order remained CLI, fresh Python, resident Python, so helper/cache/order effects are not controlled by a balanced crossover.

| Public diagnostic ID | Bundled CLI extraction completion | Bundled CLI five-second PCM completion after decoder launch | Fresh Python | Resident Python |
| --- | --- | --- | --- | --- |
| `jNQXAC9IVRw` | 5,549 ms, selected format 140 | 601 ms; 960,000 bytes | Extractor error | Extractor error |
| `BaW_jenozKc` | Extractor error after 2,307 ms | No PCM | Extractor error | Extractor error |
| `aqz-KE-bpKQ` | 4,452 ms, selected format 258 | 3,059 ms; 960,000 bytes | Extractor error | Extractor error |

The bundled CLI had two identity matches and two complete PCM excerpts out of three attempts. Each Python arm had **zero identity/PCM successes out of three**. Their shorter median wall times (2,895 ms fresh, 2,281 ms resident) are times to failure, not startup improvements. The generic `extractor_error` classification does not distinguish source-package/dependency/plugin incompatibility from provider denial. The test clip's availability failure is not proof that a requested music recording is unavailable.

This probe records **five-second PCM completion, not first-PCM latency**. `processMs` on CLI rows ends after extraction, whereas Python process/request wall fields include the probe when reached; the report's combined `medianProcessOrRequestMs` field must not be used for a cross-arm startup comparison. Successful latency summaries must preserve these boundaries and failures. No complete CPU/RSS accounting, mixer/device timing, 20-repetition qualification, or Python reuse performance result follows from this run.

The harness was corrected before the matrix to drain bounded protocol frames, preserve normal HOME with isolated task XDG paths, explicitly disable account credentials, retain format IDs/allowlisted headers, checkpoint attempts, and terminate owned process groups. Resident FFmpeg shares its worker's group so a parent deadline reaches it; cleanup also reaches children after their leader exits. Offline lifecycle smoke checks exercised incomplete-frame deadline return and exited-leader cleanup, and Python compilation passed. These are targeted harness checks, not production scheduling or all-platform cancellation tests.

A bounded root-cause follow-up found a concrete packaging gap. The worker's `python -S` hides site packages, including `yt_dlp_ejs`. The matching source expects EJS **0.8.0**; its archive includes the core solver and Bun/Deno libraries, but lacks Node's generic solver library. [Pinned solver implementation](https://github.com/yt-dlp/yt-dlp/blob/897341241f2bfb2439fe0367f4c7c9ccd29a3828/yt_dlp/extractor/youtube/jsc/_builtin/ejs.py), [version/hash allowlist](https://github.com/yt-dlp/yt-dlp/blob/897341241f2bfb2439fe0367f4c7c9ccd29a3828/yt_dlp/extractor/youtube/jsc/_builtin/vendor/_info.py). The installed EJS 0.8.0 minified core and library scripts both exactly matched that allowlist. Copying only that package to isolated scratch supplied the missing dependency without adding a production runtime or reading user credentials.

One [follow-up Python attempt](performance-results/2026-10-19-extractor-reuse/python-diagnostic.json) on the previously successful `jNQXAC9IVRw` then matched the identity and produced 960,000 PCM bytes: extraction **4,941 ms**, five-second PCM completion **403 ms**, worker initialization 216 ms; the request had a 20-second cap with separate warm-helper setup. It selected **format 18**, compared with the earlier CLI's format 140. This is evidence that supplying the matching challenge dependency makes the source-based Python path executable, not proof that it alone explains every original failure or that formats/runtimes are equivalent. The original nine rows remain unchanged. See the [experiment notes](performance-results/2026-10-19-extractor-reuse/README.md) for scope. Further public repeats stop here; establish matching dependency and format behavior before comparing fresh and resident performance.

After capture, the harness added fixed allowlisted failure categories for missing module/runtime, challenge/token, format, authentication, rate limiting, denied access, transport and deadline failures. Error text is inspected in memory; emitted types and stages are allowlisted, and signed URLs or credentials are never copied into diagnostics. This improves future diagnostics without retrospectively reclassifying the captured nine rows. Four [offline protocol tests](../scripts/performance/extractor_reuse_experiment_test.py) pass: secret-bearing exceptions cannot cross the protocol, dependency/provider classifications stay distinct, incomplete frames honor an absolute deadline, and oversized frames are capped. Run them with `python3 -m unittest scripts/performance/extractor_reuse_experiment_test.py`; no public requests are made by these tests.

## Direct YouTube Music native playback: Meld comparison

**Direct native HTTP does not eliminate token, cipher or session setup.** Meld avoids a provider playback page for ordinary decoding, but its Android token provider explicitly uses a WebView. This corrects any broader impression that its native player obtains a ready media URL from one uncomplicated Music API call. A desktop design here must keep the existing renderer-free BgUtils process boundary and yt-dlp fallback; copying Meld's Android token implementation would introduce a different dependency.

### Pinned source evidence

The comparison re-fetched public source at Meld `1ded7122ee0bd166bd703bfe3b63d233af8b7dcd`, the revision retained by the earlier review, rather than asserting it is today's latest HEAD. Its version catalog pins InnerTubeX `v0.5.2`; a read-only tag lookup resolves the annotated tag to commit `93b650b877530738cbc4d2183b4322b77bb9033e` (tag object `a680b2fc9bfc52005b85a38f555ac2954463850e`). The dependency source archive SHA-256 is `ba9519a7851131aa93094c4b5a0dfb1ee238f8a0975a85274c9892cb412cbcc2`. Sources were read in scratch; no Android application, account or provider player was launched.

| Component inspected | Concrete behavior and relevance |
| --- | --- |
| [Meld `InnerTubeXPlayer`](https://github.com/FrancescoGrazioso/Meld/blob/1ded7122ee0bd166bd703bfe3b63d233af8b7dcd/app/src/main/kotlin/com/metrolist/music/utils/InnerTubeXPlayer.kt) | Retains an extractor/cipher/config bundle by transport generation. Supplies content/client capabilities, excludes failed clients, rejects unsupported SABR/HLS, and exposes stream URL, headers, expiry, client and range requirements. Source SHA-256 `3ebc136979dbdf4c5b8f0e3c64f1b3f9b6c0bd7d10756cabbcfd7a2921b2db20`. |
| [Meld token generator](https://github.com/FrancescoGrazioso/Meld/blob/1ded7122ee0bd166bd703bfe3b63d233af8b7dcd/app/src/main/kotlin/com/metrolist/music/utils/potoken/PoTokenGenerator.kt) | Owns an Android token WebView, an eight-second timeout, session-bound state, and expiry/dead-renderer recreation. Mints a session-bound token before a per-video token. Its variable names differ from the returned context slots, so copy neither names nor binding assumptions blindly. SHA-256 `d4b377cdd78839860f692ea807e784f8d03ccb7ae93fa486ad557c535ab70d15`. |
| [InnerTubeX token contract](https://github.com/MetrolistGroup/innertubex/blob/93b650b877530738cbc4d2183b4322b77bb9033e/src/commonMain/kotlin/com/metrolist/innertubex/extraction/TokenProvider.kt), [client director](https://github.com/MetrolistGroup/innertubex/blob/93b650b877530738cbc4d2183b4322b77bb9033e/src/commonMain/kotlin/com/metrolist/innertubex/extraction/PlayerClientDirector.kt) | Separates player-request and streaming-data tokens and declares provider/WebView capabilities. The director maps video-ID binding to the streaming-data token and visitor-data binding to the player-request token. This distinction was absent at initial prototype inspection and is tested by the corrected-binding checkpoint below. |
| [InnerTubeX cipher service](https://github.com/MetrolistGroup/innertubex/blob/93b650b877530738cbc4d2183b4322b77bb9033e/src/commonMain/kotlin/com/metrolist/innertubex/cipher/YouTubeCipherService.kt) | Uses QuickJS, cached player code/solvers, remote player configuration and EJS fallback. A native HTTP transport still needs a maintained JavaScript/cipher implementation and player-version invalidation. |
| [InnerTubeX extractor](https://github.com/MetrolistGroup/innertubex/blob/93b650b877530738cbc4d2183b4322b77bb9033e/src/commonMain/kotlin/com/metrolist/innertubex/extraction/InnerTubeExtractor.kt) | Caches player config with session identity, checks session changes during fetch, and coordinates client fallback, cipher and tokens. Its optional prewarm itself performs setup; it is not evidence that setup is free or that Burrata's unsuccessful prewarm A/B should be promoted. |
| [Meld native handoff](https://github.com/FrancescoGrazioso/Meld/blob/1ded7122ee0bd166bd703bfe3b63d233af8b7dcd/app/src/main/kotlin/com/metrolist/music/playback/MusicService.kt) | Resolves through the extractor, caches URL with headers/expiry/generation and range policy, then hands the result to Media3's data source. Native decoding is separate from Spotify-to-YouTube matching. Burrata should retain its own decoder and stricter recording gates. |

Meld's transport facade carries visitor/session state and rebuilds its extraction transport on changes. These are architectural examples, not a qualified desktop implementation or proof of anonymous success on every track. Android client availability, QuickJS integration and token WebView behavior do not automatically transfer to Linux/macOS/Windows. [Pinned facade](https://github.com/FrancescoGrazioso/Meld/blob/1ded7122ee0bd166bd703bfe3b63d233af8b7dcd/innertube/src/main/kotlin/com/metrolist/innertube/InnerTube.kt), [dependency pin](https://github.com/FrancescoGrazioso/Meld/blob/1ded7122ee0bd166bd703bfe3b63d233af8b7dcd/gradle/libs.versions.toml).

### Existing Burrata path and missing work

Production [`youtube_music.rs`](../src-tauri/src/youtube_music.rs) already performs a bounded native `WEB_REMIX` catalog search. It returns possible recording locators; it does **not** extract playable media. Fresh yt-dlp metadata/identity and access checks still authorize playback. Keep this distinction when comparing search speed with stream readiness.

The isolated youtubei.js worker already creates a reusable anonymous session/player, runs cancellable HTTP requests, deciphers a selected audio URL, handles signed expiry, retries denial once and decodes five seconds through FFmpeg. It is a direct InnerTube experiment, but defaults to `WEB`; its allowlist has `MWEB`, `ANDROID`, `IOS` and `TV`, not a dedicated `WEB_REMIX`/`ANDROID_MUSIC` playback strategy. Its search uses general video search, not the production Music locator policy.

Before treating it as a direct Music resolver, concrete missing steps are:

1. Preserve the exact Music locator/provider ID and require a returned upstream video ID. Currently `core.mjs` rejects a conflicting ID only **if present**; request/expected-ID equality alone does not establish returned identity. Its `recordingVerified` field is scoped to the explicit ID experiment and must never bypass production recording/version/credit checks.
2. Map direct response metadata into the existing canonical/alias/catalog gates with complete artist/title/version/duration evidence. Do not manufacture yt-dlp-shaped proof or replace fresh proof with a successful HTTP response.
3. Qualify the separated token contexts with correct video/session/client binding, refresh and generation invalidation. The corrected-binding success below advances this step but covers only one explicit source/client. Do not transplant Meld's token WebView or use account cookies to make an anonymous test pass.
4. Retain a maintained cipher/player implementation, bound player-code evaluation in an owned process, and invalidate state after player/client/tool changes. Measure initialization, challenge solving and network fetch separately.
5. Add diagnostic format-ID pinning and report the actual itag, codec/container, headers policy and range requirements. A post-success change now emits the chosen itag, but `chooseFormat(audio, best, any)` cannot pin the same stream as yt-dlp, and its decoder receives a URL without the production header handoff.
6. Return an owned prepared source through existing validation and native decoder boundaries. Keep expiry margin, range/body checks, URL/header redaction, cancellation, account isolation and queue generations; do not implement an embedded provider player.
7. Attempt direct resolution only within the original request budget, reserving time for existing yt-dlp fallback. A proposed experimental partition is at most six seconds for direct extraction while leaving at least eight seconds for fallback/validation; measure misses and duplicate work before choosing production numbers. Both routes must share attempt limits and no stale direct result may activate after fallback wins.

### Bounded experiment proposal and evidence limit

The parallel read-only prototype audit found one no-edit capability probe feasible: fixed official Matthew upload `uZGRATckoBM`, `MWEB`, anonymous bundled BgUtils, five seconds PCM, a 19-second worker budget and 20-second supervising wall deadline. The exact owned-process command was sent to the release coordinator before execution. It uses bundled Node/FFmpeg and disposable session/XDG paths, preserves normal HOME, suppresses stderr and emits only the worker's structured result. This measures that explicit upload's direct extraction/decipher/decode capability; it does not establish a dedicated Music client or matched-format performance. Existing expected-ID failures and Python/JSONL experiments remain separate and unchanged.

The coordinator executed that one public-network attempt on 2026-10-19. It returned **`STREAM_DENIED` at decode**, with no successful five-second PCM result and incomplete resource accounting. The exact redacted response from `/tmp/burrata-014-matthew-direct-capability.json` is preserved here:

```json
[
  {
    "id": "matthew-direct-01",
    "ok": false,
    "failure": "STREAM_DENIED",
    "stage": "decode",
    "stages": {
      "session": 4277.917993,
      "sessionWait": 4277.660987,
      "token": 6542.659909,
      "extraction": 670.4464540000008,
      "format": 0.7648460000000341,
      "decipher": 61.81095000000096,
      "decode": 906.6852460000009
    },
    "accountingComplete": false
  }
]
```

Stage durations are milliseconds. `sessionWait` overlaps session initialization and must not be added as a separate serial cost. Token/extraction/decode stage values can include the prototype's bounded denial refresh; they are aggregate stage durations, not per-request network counts. This result reached deciphering but did not establish a playable selected stream, recording/version equivalence or first-PCM latency. Session and token work are visible setup costs; the 670 ms extraction field alone is not a playback startup measurement. One denied attempt establishes neither universal provider failure nor faster coverage than yt-dlp. The later binding correction is a separate checkpoint, not a replacement for this failure.

### Corrected-binding capability checkpoint

The experimental token adapter subsequently separated the mweb player-request token bound to session visitor data from the GVS token bound to the explicit video ID. One [preserved corrected-binding attempt](../scripts/performance/innertube/results/2026-10-19-matthew-corrected-binding.json) then returned **240,000 frames / five seconds PCM** for `uZGRATckoBM`, codec `audio/webm; codecs="opus"`, without a denial refresh. First PCM was **11,837.69 ms from request-line receipt**; process launch is excluded. Session initialization took 5,214.70 ms, token work 5,829.17 ms, extraction 331.21 ms, decipher 32.89 ms and decoder-first-PCM 426.81 ms. Resource accounting reported complete for this excerpt. The slow cold session/token setup remains visible even though metadata extraction itself is short.

The captured row does not contain its itag, so it cannot establish same-format parity with the earlier yt-dlp results. A post-capture change adds a safe `formatId` field for subsequent probes; its updated source hash does not retroactively identify the captured format. The [companion note](../scripts/performance/innertube/results/2026-10-19-matthew-corrected-binding.md) preserves the result boundary. This is explicit-provider-ID feasibility, not full catalog/acoustic recording/version proof, native mixer/device playback or a controlled speed improvement. The earlier denied attempt remains retained.

The proposed three-request reuse probe keeps one worker for same-ID repetition and then a different public ID. If all JSONL lines are supplied immediately, queue waiting consumes each command's budget; later rows do not receive a new full budget when execution starts. A sequential request dispatcher is required to compare independent per-request latency without that queue-time confound. Such a reuse probe and the corrected three-arm benchmark below answer different questions; do not merge their sample populations.

A controlled next comparison needs a fixed public ID **and** the same itag/container/headers policy on direct and yt-dlp arms, both through the same decoder. Record cold session, reused session and seek separately, with request-to-first-PCM and five-second completion distinguished. First run one feasibility pair and retain denial/missing-token/cipher/identity failures. Only after both decode the matched stream should three exploratory alternating pairs be considered; no 20-run matrix or faster-playback claim is justified yet. No direct-resolver production code, dependency or account changes were made by this research.

## Corrected matched extractor benchmark

The next diagnostic checkpoint adds `--matched` to the existing Python harness while preserving the prior raw reports and legacy command behavior. This path fixes the earlier all-format reselection and mismatched timing boundaries. It pins an exact video ID and numeric itag in both CLI/API selectors, verifies the **top-level selected** ID/itag before decoding, and never substitutes another entry from the available-format list. Unknown/mismatched identity or format is a typed failure. Pair comparisons also require matching codec, container, protocol, sample rate, channels and available content length; the report retains exclusions rather than calling them improvements.

The three arms are bundled fresh CLI, fresh exact-source Python process/object, and one serialized resident Python process/object. Both Python arms load the same pinned release source and hash-verified EJS dependency under `python -S`; system/user packages and Python path overrides are excluded. The frozen executable retains its own runtime/dependency graph, so the primary isolated reuse comparison is fresh Python versus resident Python. CLI comparisons remain a contextual bundled-tool control. All arms use mweb, the same token-provider policy, one format ID, the same allowlisted-header handoff and the same bundled FFmpeg function.

`requestToFirstPcmMs` starts in the parent before each arm's setup and ends at the first decoded PCM bytes, using the host's monotonic clock in both processes. A first resident request includes worker spawn/readiness; subsequent requests retain the object. Extraction completion, decoder-first-PCM and five-second completion have separate fields. A shared anonymous BgUtils helper is warmed once; its setup duration is reported separately and excluded from per-arm timing. This isolates extractor reuse and does not measure whole-application cold startup, native mixer/device output or full descendant CPU/RSS.

Three blocks rotate arm order: CLI/fresh/resident, fresh/resident/CLI, resident/CLI/fresh. Each arm receives one absolute 20-second deadline, with 240 seconds for the entire run. Worker requests carry an epoch, request ID and deadline; stale protocol frames cannot satisfy another request. The resident worker exits on EOF or a 60-second idle deadline and is retired on timeout/protocol failure. Owned process-group cleanup reaches its decoder and JS descendants. Each loopback gate now owns a distinct handler/token; constructing another arm's gate cannot overwrite the resident gate. Existing output files are refused rather than replaced.

Offline validation passed **12 tests**, including exact selected-stream rejection, sensitive-field/header allowlists, failure-excluding summaries, Latin ordering, independent gate ownership, incomplete/oversized frame limits and fingerprint mismatch exclusions. A real generated local five-second WAV produced exactly 960,000 PCM bytes with first PCM measured before completion. Separate exact-source worker checks rejected a stale epoch and exited on idle without public requests. A parallel Luna read-only audit found no matching/timing/lifecycle blocker; startup import failures still report a fixed readiness failure rather than exposing raw stderr.

The bounded live command sent to the coordinator is:

```bash
python3 scripts/performance/extractor_reuse_experiment.py \
  --matched --run-live \
  --source-root /tmp/burrata-extractor-reuse/yt-dlp-source/release \
  --source-archive /tmp/burrata-extractor-reuse/yt-dlp-source/yt-dlp-release.tar.gz \
  --dependency-root /tmp/burrata-matched-extractor-deps \
  --video-id uZGRATckoBM --format-id 251 --repetitions 3 \
  --budget-seconds 20 --total-budget-seconds 240 --idle-seconds 60 \
  --output docs/performance-results/2026-10-19-extractor-reuse/matched-matthew-251.json
```

The scratch source archive and isolated EJS package are the previously verified assets; the non-live mode checks their pins before any provider request. This command is nine exploratory attempts, zero warmups, not a 20-run qualification. Shorter failure time does not count as successful first PCM.

### First matched matrix and CLI command correction

The [first matched matrix](performance-results/2026-10-19-extractor-reuse/matched-matthew-251.json) completed all nine attempts in 43,722 ms, with helper setup 2,512 ms separately reported and helper shutdown observed. Every successful selection was the exact official Matthew ID, itag 251, Opus/WebM, HTTPS, stereo 48 kHz, content length 3,845,541 bytes.

| Block | Fresh CLI | Fresh Python | Resident Python |
| --- | --- | --- | --- |
| 1 | Command failure before extraction | Five-second PCM; first PCM 7,292 ms | Stream denied at decoder |
| 2 | Command failure before extraction | Stream denied at decoder | Five-second PCM; first PCM 5,274 ms |
| 3 | Command failure before extraction | Requested format unavailable | Stream denied at decoder |

There is **no successful planned comparison pair** (CLI versus fresh Python, or fresh versus resident Python): the Python successes occurred in different blocks, and the CLI attempts failed before provider extraction. The successful individual latencies establish same-ID/itag decoding capability in those attempts; subtracting them would confound request conditions and ignored failures. No reuse-speed improvement follows.

An offline investigation identified a concrete harness mistake: the pinned CLI does not support `--no-netrc`. It exited with a usage error, while the initial safe classifier retained only `EXTRACTOR_ERROR`. These three rows are therefore command failures, not evidence that the CLI cannot play Matthew. Running `--version` had concealed the problem because it exits before unknown-option validation. The raw matrix and captured source hash remain unchanged.

The diagnostic command builder now omits that unsupported switch; netrc remains disabled by default because it is opt-in and user config is ignored. Future command errors get fixed category `CLI_USAGE_ERROR`. The harness also validates the complete configured option set through **`--list-extractors` with no URL** before any public request; this catches unsupported options without using provider traffic. All 12 offline tests still pass, and the corrected full-option preflight passes. Any rerun must use a new output path, retain this failed matrix, and stay serialized with the separate direct-InnerTube probe. No additional live attempt was made during the CLI investigation.

### Corrected command matrix: failures still prevent a reuse conclusion

The [corrected-command matrix](performance-results/2026-10-19-extractor-reuse/matched-matthew-251-cli-fixed.json) subsequently completed nine attempts in **63,981 ms**, with a separately excluded **2,733 ms** helper setup. All arms requested `uZGRATckoBM`, mweb, exact itag 251 with no format fallback, the same allowlisted headers policy and the same FFmpeg decoder. Both Python arms used the same source/EJS dependency setup. The frozen CLI still has its own runtime/dependency graph; equal request flags and source pin do not make those graphs identical.

| Block | Fresh CLI | Fresh Python | Resident Python |
| --- | --- | --- | --- |
| 1 | Five-second PCM; first PCM **8,893 ms** | Stream denied | Five-second PCM; first PCM **6,027 ms**, including initial worker readiness |
| 2 | Format unavailable | Five-second PCM; first PCM **7,083 ms** | Stream denied |
| 3 | Stream denied | Stream denied | Stream denied |

Each arm succeeded **once in three attempts**. The successful streams have equal fingerprints: ID/itag, Opus/WebM, HTTPS, 48 kHz, two channels and 3,845,541-byte content length. There is still no successful planned CLI/fresh-Python or fresh/resident-Python pair. Block 1 contains both CLI and resident successes, but that resident request was its initial object use, and the runtime graphs differ; it does not isolate reuse. The successful Python values in different blocks cannot be subtracted into a speed result. All failed samples remain in the denominator and all planned paired deltas are excluded.

The CLI command problem is fixed; these newer `FORMAT_UNAVAILABLE` and decoder `STREAM_DENIED` outcomes are separate from the old usage failures. A format may be absent from one fresh response, or an extracted URL may be denied at decode despite an equal metadata fingerprint. The report does not establish whether token attestation, transient CDN access, client/session state or another provider condition caused each denial. No timeout occurred, helper shutdown was observed, and the resident was reused without startup on later requests. This is a bounded failed qualification of Python reuse, not evidence for promotion. Further Python live matrices stop until a concrete provider/access fix is identified.

## Measured direction: persistent direct worker

The coordinator's [sequential same-worker probe](../scripts/performance/innertube/results/2026-10-19-matthew-same-worker-reuse.json) supplies a more useful positive feasibility result. Requests were dispatched after the prior response, so queue wait did not consume the following request's independent 20-second budget. All three excerpts decoded five seconds and reported complete excerpt resource accounting:

| Request | Observed selected format | Request-line receipt to first PCM | State |
| --- | --- | --- | --- |
| First Matthew request | 251, Opus/WebM | **9,728 ms** | Cold worker session/token state; process launch excluded |
| Same Matthew request again | 251, Opus/WebM | **372 ms** | Session, token and media-info cache reused |
| Different public video `jNQXAC9IVRw` | 140, AAC/MP4 | **546 ms** | Existing session/player/minter reused; new video-bound GVS token and fresh extraction |

The first request spent **5,294 ms** in session initialization and **3,690 ms** in token work. The repeated Matthew request had an info/token cache hit; the different-ID request had an info/token cache miss but token work completed in **1.4 ms**, with fresh extraction **118.9 ms**. The latter indicates that reusable minter/session/player machinery can help a new ID, rather than only replaying an old signed URL. These are three observations in one warm sequence, not independent repetitions, a controlled CLI comparison, a latency percentile or a full-track/device test. The third request uses a different recording and codec. This supports the next experiment direction without claiming a general speedup or catalog/acoustic identity proof.

The next bounded implementation target should therefore be a **persistent direct InnerTube worker with proper player/GVS binding**, keeping yt-dlp fallback. Its output must require and retain the actual returned provider ID, fresh recording metadata and selected format, alongside ephemeral URL/headers/expiry and session generation. Adapt that evidence into existing recording/version gates and native validation/decoder ownership; never use the experimental `recordingVerified` boolean as a production gate. Preserve one absolute foreground deadline, owned child groups, idle retirement and cancellation/account invalidation. Fallback must use the same selected recording and remaining budget, with failures retained.

Before promotion, prove a fresh different-ID request, seek, expiry refresh, denial recovery, cancellation/supersession, bounded idle/RSS and packaged dependency behavior on supported targets. The successful warm samples justify this focused work; they do not justify skipping those checks or releasing an unqualified replacement. Existing reader integration, early-batch verification and queue-preparation experiments remain independent opportunities. No production dependency or direct-resolver path was promoted by these research results.
