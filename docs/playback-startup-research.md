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

The Linux tool lock pins yt-dlp `2026.10.10.233141`, upstream source commit `8973412` (the lock records this abbreviated revision), executable SHA-256 `60606cef1a65d70f6639a85b45ebb7acf08dbb81831e134dd93939e23ca2b792`, Node `24.21.0`, and FFmpeg `8.1.3`. BgUtils provider is `2.0.2`, revision `26475e9d3665b972c4aeb83b4dbaafcf64b88438`. The isolated Node prototype uses youtubei.js `18.1.0` and its checked-in npm integrity lock. Sources below use those revisions/tags where available; current upstream documentation is explicitly separate from shipped behavior.

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

The local experimental `token.mjs`/`token_child.mjs` always bind to `visitorData`. That is not demonstrated equivalent to production's context-dependent binding. Before arm D can be qualified, implement and test the correct request-context/video binding, bypass-cache refresh on denial, and independent player/GVS handling where required. Existing offline tests verify its local protocol, not live provider compatibility. youtubei.js v18.1.0 supports session caching with version checks and player caching keyed by player ID/library version, so reusable state is feasible within its API. [Session source](https://raw.githubusercontent.com/LuanRT/YouTube.js/v18.1.0/src/core/Session.ts), [Player source](https://raw.githubusercontent.com/LuanRT/YouTube.js/v18.1.0/src/core/Player.ts). These facilities do not establish parity with the bundled yt-dlp clients or token providers.

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
| Validation and reader | `validate_prepared_preview_audio` checks DNS and bounded byte ranges, including parallel start/tail for known large lengths. Native decode then opens a separate `StreamDownload` reader. | Prototype validation that hands the same owned reader/buffer to decode. Measure requests and first PCM against existing separate preflight; retain preactivation checks and tail/length guarantees. |
| Next queue item | `prepare_next` prefetches stable candidates early, waits until at most 30 seconds remain before resolving expiring URLs/decoder, and checks queue generation/cancellation. | Compare existing 30-second window with 45 seconds on unchanged queue fixtures. Measure inter-track gap, success, abandoned preloads, URL expiry, CPU/RSS and network requests. |

Reader integration must reject truncated or oversized 206 bodies, inconsistent Content-Range/length/MIME, identity mismatch and stale session epochs before activation. Seeking must preserve range support and cancellation while headers are pending. One reader may retain validated bytes; it must not turn a full GET or decoder allocation into an unbounded preflight. Queue replacement, repeat/reorder, pause, supersession and account reset must prevent stale activation and release resources. Validate cancellation of a joined cache waiter without destroying another request's valid flight.

## Reproducible reporting and promotion boundary

Extend the existing machine-readable experiment reports with `experiment`, `arm`, `pairId`, `iteration`, `warmup`, input/tool/source hashes, session policy, absolute budget, provider preference/grace, candidate batch timestamps, gate class, selected public identity, extraction attempts, cache state, expiry margin, first PCM, typed outcome and descendant accounting. Keep `qualification: false` until native/device application scenarios and supported targets are independently exercised. Failures remain rows; summaries exclude warmups and show successful sample count plus failure denominators. A first-PCM diagnostic is decoder-only unless a real mixer/device boundary is measured.

First deliver the batch scheduler's offline delayed-result tests and an isolated worker protocol fixture. Run the bounded exploratory public comparisons only after those pass. Promote neither overlap nor worker reuse solely on a helper readiness reduction: require unchanged recording/version behavior, no additional deadline or cancellation failures, clean helper shutdown, and repeatable end-to-end improvement on a same-input matrix. This research adds no production runtime code, dependencies, profile changes or new live qualification results.
