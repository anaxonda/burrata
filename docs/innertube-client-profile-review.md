# Pinned native client profiles and the VISIONOS control

This is diagnostic research, not a production resolver change or a release qualification. The anonymous `VISIONOS_0_1` control for Matthew returned HTTP 200, `UNPLAYABLE`, and no available audio formats in **318.16 ms**. It did not reach decoding. The [original safe report](../scripts/performance/innertube/results/2026-10-19-visionos-01-matthew-251-initial-control.json) is preserved byte-for-byte, SHA-256 `58af45592f6ff0ccffc99778f5fceb7ca9068afc9dd82b3e59ce54100ee8f014`. A fast rejection is not a startup improvement, nor proof that the profile fails for every recording or network.

The report's `sourcePins.meld` records the comparison revision declared at capture, `1ded7122ee0bd166bd703bfe3b63d233af8b7dcd`. That declaration does not establish complete request parity with that Meld revision. A later independent audit found the review checkout at `9c3dfc8618625a94e7019fbb0f3e31f2c87f74e9`; the older raw facade copy is byte-identical, but its original fetch provenance was not retained in a machine-readable manifest. The corrected request therefore explicitly combines the verified InnerTubeX profile with the separately pinned **9c3** Meld application defaults. It does not relabel the original result.

The root agent subsequently ran v2 once for each corpus ID under the bounded driver. All three returned HTTP 200, `UNPLAYABLE`, `UNCLASSIFIED_REJECTION`, and no audio formats; none reached decode. The reports are preserved unchanged:

| Explicit source | Probe wall time | Retained report |
| --- | --- | --- |
| Matthew | 367.13 ms | [v2 Matthew](../scripts/performance/innertube/results/2026-10-19-visionos-v2-matthew-251.json) |
| Poso lyric upload | 392.48 ms | [v2 Poso](../scripts/performance/innertube/results/2026-10-19-visionos-v2-poso-251.json) |
| Timmy Topic candidate | 360.24 ms | [v2 Timmy](../scripts/performance/innertube/results/2026-10-19-visionos-v2-timmy-251.json) |

These results show that adding the two application headers did not make these three nullable-visitor requests playable under the tested conditions. They do not establish that VISIONOS is generally unsupported or that Meld's prepared visitor-context path fails.

## Source and request audit

The InnerTubeX v0.5.2 archive resolves to commit [`93b650b877530738cbc4d2183b4322b77bb9033e`](https://github.com/MetrolistGroup/innertubex/tree/93b650b877530738cbc4d2183b4322b77bb9033e); archive SHA-256 is `ba9519a7851131aa93094c4b5a0dfb1ee238f8a0975a85274c9892cb412cbcc2`. The [source-derived golden fixture](../scripts/performance/innertube/fixtures/visionos-01-source-request.json) retains canonical excerpts and complete-file hashes for the client model, context DTO, player body, request construction, and Meld facade. The expected request was written from those sources independently of the probe builder, then compared against it in offline tests.

| Request element | Source result and probe treatment |
| --- | --- |
| Identity | `VISIONOS`, version `0.1`, numeric client ID `101`; exact Safari 17.5 user agent, `VISION_OS` 1.3, Apple `RealityDevice14,1`. This is a different profile from VISIONOS 1.02. |
| Platform | **Retain `platform: "MOBILE"`.** The verified client model sets it and `toContext` passes it. An intermediate source-reading error suggesting its omission was corrected before any edit or live repeat. Model SHA-256 `694d66a4c8dc4f9c67b1abc971835ddbd5d09022de8b286a986ad5039b846d90`. |
| Endpoint | Music `/youtubei/v1/player?prettyPrint=false`; Music Origin, X-Origin and Referer. |
| Body defaults | Meld uses `encodeDefaults=true`, `explicitNulls=false`: include request SSL/experiment defaults and safety mode; omit null visitor data, account identity, token, playback context and videoCheckOk. No player JS or signature timestamp is needed by this declared profile. |
| Headers | The initial control omitted Meld-wide `Accept: application/json` and `Cache-Control: no-cache`. Construction `innertubex-93b-profile-meld-9c3-defaults-v2` adds them; existing profile headers remain. The verified [9c3 facade](https://github.com/FrancescoGrazioso/Meld/blob/9c3dfc8618625a94e7019fbb0f3e31f2c87f74e9/innertube/src/main/kotlin/com/metrolist/innertube/InnerTube.kt) SHA-256 is `3d981b55d4e47a49bb0dca53df0eb9bfd659f0abe2f356059da983ecde30f3a1`. |
| Transport | Node fetch is not Ktor/OkHttp: compression, connection reuse, retry and HTTP version behavior differ. `transportParity:false` is explicit. No account cookies, authorization, visitor context, token or provider player is introduced by v2. |
| Media handoff | Pinned Meld omits its usual playback headers for VISIONOS. The probe passes an empty media header map to the existing decoder, rejects cipher and n-transform requirements, pins the requested itag without fallback, and does not attempt HLS/SABR. |

Sources: [client/context construction](https://github.com/MetrolistGroup/innertubex/blob/93b650b877530738cbc4d2183b4322b77bb9033e/src/commonMain/kotlin/com/metrolist/innertubex/models/YouTubeClient.kt), [request construction](https://github.com/MetrolistGroup/innertubex/blob/93b650b877530738cbc4d2183b4322b77bb9033e/src/commonMain/kotlin/com/metrolist/innertubex/InnerTube.kt), and [media handoff](https://github.com/MetrolistGroup/innertubex/blob/93b650b877530738cbc4d2183b4322b77bb9033e/src/commonMain/kotlin/com/metrolist/innertubex/extraction/InnerTubeExtractor.kt).

## Visitor context is a justified separate experiment

Visitor data is nullable in the profile. The director's initial untokenized player attempt does **not** require a fresh visitor lookup. However, the ordinary extractor obtains cached or fresh watch configuration first, and the director selects session visitor data or `PlayerConfig.visitorData`. When present, the same value reaches both `context.client.visitorData` and `X-Goog-Visitor-Id`. Meld prewarm can also obtain anonymous visitor data before preparing its config. Consequently, the control's omitted visitor data is a supported reduced request, but it does not reproduce Meld's complete preparation lifecycle. [Extractor](https://github.com/MetrolistGroup/innertubex/blob/93b650b877530738cbc4d2183b4322b77bb9033e/src/commonMain/kotlin/com/metrolist/innertubex/extraction/InnerTubeExtractor.kt), [director](https://github.com/MetrolistGroup/innertubex/blob/93b650b877530738cbc4d2183b4322b77bb9033e/src/commonMain/kotlin/com/metrolist/innertubex/extraction/PlayerClientDirector.kt).

An explicit `visitorMode: "anonymous-source"` arm now follows the pinned anonymous GET to `https://www.youtube.com/sw.js_data`, using the source's Firefox 140 web user agent, locale, Accept policy and separately verified Meld Cache-Control default. It parses the anti-XSSI response's `[0][2][0][0][13]` value; if unavailable, it makes the source's one Music-homepage fallback and uses the exact `VISITOR_DATA`/`visitorData` regex. It does not fetch a watch page, player JavaScript or PO tokens. The [visitor source fixture](../scripts/performance/innertube/fixtures/visionos-01-anonymous-visitor-source.json) retains the canonical method excerpt, complete source-file hash, user-agent declaration and expected lookup requests. [Pinned visitor flow](https://github.com/MetrolistGroup/innertubex/blob/93b650b877530738cbc4d2183b4322b77bb9033e/src/commonMain/kotlin/com/metrolist/innertubex/InnerTube.kt).

Construction `innertubex-93b-profile-meld-9c3-anonymous-visitor-v3` marks this separate arm; default v2 omission behavior remains available. Each lookup uses `credentials:omit`, redirect rejection, a four-MiB response cap and the original absolute command deadline. At most two context GETs precede the single player POST; cancellation/deadline prevents further fallback or player work. The same fresh value is sent in the player header and client context, retained only in memory. JSONL callers cannot supply visitor identifiers or account state. Reports retain source labels, HTTP statuses, header names and a found boolean; they never retain the visitor value or server body. The probe additionally requires a printable ASCII visitor value of at most 4,096 bytes. Source session publication/caching, transient retries and OkHttp behavior are deliberately absent from this fresh-process diagnostic and remain outside its parity claim.

The source audit found no blocking construction mismatch and confirmed the intentional transport/validation differences above. The root agent then ran three fresh v3 visitor-mode requests. Each used service-worker context, returned the exact requested ID, passed the expected title/channel/duration comparison, selected itag 251 Opus/WebM, and decoded five seconds of PCM:

| Explicit source | Request-handling to first PCM | Returned duration | Declared selected content length | Retained report |
| --- | --- | --- | --- | --- |
| Matthew | 863.283 ms | 228 s | 3,845,541 bytes | [initial v3 Matthew](../scripts/performance/innertube/results/2026-10-19-visionos-v3-visitor-matthew-251-initial.json) |
| Poso lyric upload | 1,154.755 ms | 250 s | 4,316,912 bytes | [initial v3 Poso](../scripts/performance/innertube/results/2026-10-19-visionos-v3-visitor-poso-251-initial.json) |
| Timmy Topic candidate | 2,664.014 ms | 288 s | 4,683,906 bytes | [initial v3 Timmy](../scripts/performance/innertube/results/2026-10-19-visionos-v3-visitor-timmy-251-initial.json) |

This is three exploratory successes with source context enabled, following three v2 rejections without it. It is not a randomized attribution study, a broad reliability sample, full-recording proof, a matched comparison with MWEB, or a complete reproduction of Meld's transport. The timer excludes process launch. Declared manifest lengths are not measured CDN bytes. No production resolver changed.

The retained report cannot distinguish region, login or client compatibility: it deliberately does not retain the provider reason. Future reports inspect reason text only in memory and emit fixed conservative categories. The pinned classifier recognizes age and general unavailability predicates and treats `LOGIN_REQUIRED` separately; it does not establish a region/client distinction. Unknown rejections remain `UNCLASSIFIED_REJECTION`. No historical result is reclassified. [Classifier](https://github.com/MetrolistGroup/innertubex/blob/93b650b877530738cbc4d2183b4322b77bb9033e/src/commonMain/kotlin/com/metrolist/innertubex/extraction/PlayabilityFailure.kt).

## Why this profile was worth probing

The pinned [catalog](https://github.com/MetrolistGroup/innertubex/blob/93b650b877530738cbc4d2183b4322b77bb9033e/src/commonMain/kotlin/com/metrolist/innertubex/extraction/strategy/PlaybackClientCatalog.kt) ranks VISIONOS 0.1 as experimental, automatic, priority 100 for normal songs, with no declared PO or JS requirement and direct transport. Its notes report anonymous 90-second playback and bidirectional seeks on two upstream samples; explicit results were inconsistent. Those upstream observations do not establish desktop or universal availability.

| Other pinned candidate | Remaining cost or constraint |
| --- | --- |
| WEB_REMIX, priority 70 | Direct/HLS; video-bound GVS token required for the ordinary anonymous path, response-dependent cipher/n work. Player-request token is not declared required. |
| WEB_EMBEDDED_PLAYER, priority 65 | No declared PO token, but embedded configuration/encrypted flags, signature timestamp and response-dependent JS remain. Content must be embeddable. |
| WEB_CREATOR | Account required; excluded from this anonymous experiment. |
| ANDROID_VR 1.65.10 | Probe-only in this catalog; reported CDN failure after 1 MiB makes a short excerpt insufficient evidence. |
| Other Android/iOS and TV profiles | Platform attestation, broken/probe-only state, DRM/SABR or unsupported transport constraints prevent treating them as a simple tokenless replacement. |

Meld also retains stream URLs by video and generation, caches player configuration by session identity, and reuses its extractor/cipher bundle. These caches can remove preparation from a warm request; they are not evidence of a global learned successful-client cache. Optional Qobuz resolution can bypass YouTube entirely when enabled, but it is a different source path and does not explain anonymous YouTube timing by itself. [Meld adapter](https://github.com/FrancescoGrazioso/Meld/blob/1ded7122ee0bd166bd703bfe3b63d233af8b7dcd/app/src/main/kotlin/com/metrolist/music/utils/InnerTubeXPlayer.kt), [native resolver](https://github.com/FrancescoGrazioso/Meld/blob/1ded7122ee0bd166bd703bfe3b63d233af8b7dcd/app/src/main/kotlin/com/metrolist/music/playback/MusicService.kt).

## Bounded next steps

The durability gate uses bounded `position` or `positionFraction` plus `seconds` (default five, at most 600). It derives fractional positions from the **returned** metadata duration and rejects a requested excerpt extending beyond it. Full recording requires explicit `fullRecording:true`, expected corpus metadata, zero position, and a returned duration at most 600 seconds. The decoder is allowed to reach EOF up to metadata duration plus one second, and the probe independently counts actual 48-kHz PCM frames against metadata duration ± one second. A 60-second preview, early EOF, excess decoded duration, invalid range, changed ID/format, cipher/n requirement or deadline remains a typed failure. Default excerpt thresholds are unchanged; callers cannot inject the internal minimum-duration option.

The standalone [fresh-process driver](../scripts/performance/innertube/visionos_driver.py) offers exploratory (three calls), seek (nine calls), full (three calls), combined durability (12 calls), and rotating cold modes. Combined durability performs five-second seeks at 1%, 50% and 95%, then one duration-counted full decode, for each source. This should expose early termination around a one-MiB or one-minute provider limit; it is not a raw HTTP range-contract audit, acoustic identity check, real-time soak or physical-device test. Every call gets a new Node process and temporary XDG/TMP directories with HOME unchanged, strict itag 251, a 19-second shared context/player/decode budget and a 20-second process deadline plus bounded cleanup. Context fallback remains at most the one reported Music-homepage request from the source.

```bash
python3 scripts/performance/innertube/visionos_driver.py \
  --mode durability --visitor-context \
  --output /tmp/burrata-visionos-v3-durability.json
```

The driver also offers `--mode cold --attempts 20 --visitor-context`: 20 fresh calls total, rotating three-ID order between blocks (seven/seven/six observations). It can accept up to 60 calls if a larger per-source sample is explicitly wanted; the whole run remains capped at 600 seconds. It never starts on import, refuses an existing output, checkpoints failures, bounds JSONL frames, filters output to safe fields, owns process-group cleanup, and invalidates all eligibility if source hashes change. It reports no automatic latency or production qualification. Every run needs a new output filename.

The root agent completed the [first 12-call durability run](../scripts/performance/innertube/results/2026-10-19-visionos-v3-durability.json), preserved byte-for-byte with SHA-256 `78e9afc238f5a5a41aea499a9b4aea6a2125ffe00abc77113a75d2bea8ef388c`. **11 of 12 calls were eligible:** all three full sources and eight of nine seek calls. Full decoded PCM durations were 227.786667, 250.020875 and 288.000 seconds against returned metadata durations 228, 250 and 288 seconds. These duration-counted results cross one minute for all three sources, whose selected manifests declare more than one MiB; no full-source early truncation was observed in these calls. They do not measure actual transferred bytes or prove real-time/device playback.

The sole ineligible call was Matthew at 50%, with `NETWORK_FAILED` at stage `player` after 631.682 ms of visitor lookup and 256.175 ms in the player request. It did not reach format validation or decoding, so it is not evidence of a midpoint seek failure or truncated stream. The original record does not identify the underlying socket/DNS cause and remains unchanged. Future reports now inspect error codes only and expose `ECONNRESET`, `UND_ERR_SOCKET`, `ETIMEDOUT`, `ENOTFOUND`, `EAI_AGAIN`, or `OTHER`; no error message, URL, path or raw body is emitted. Unknown rejection reasons remain separate from these transport categories.

The root agent ran one explicitly selected fresh diagnostic for the missed point. The [separate midpoint repeat](../scripts/performance/innertube/results/2026-10-19-visionos-v3-matthew-midpoint-repeat.json) passed: position 114 seconds, five seconds of PCM, exact ID/metadata/itag, and first PCM at 2,269.771 ms. Its report SHA-256 is `fcd01ceacd4ddb34e4c34f16398ea314204fe87b42709c7f66620a4a3d914236`. It does not replace the original first-attempt failure or alter the durability run's **11/12** denominator. The original failure remains unclassified beyond its player-request stage.

```bash
python3 scripts/performance/innertube/visionos_driver.py \
  --mode seek --only matthew:seek-50 --visitor-context \
  --output /tmp/burrata-visionos-v3-matthew-midpoint-repeat.json
```

The 27 offline probe tests and eight driver tests cover source goldens, visitor privacy/cancellation, shared deadlines, duration and preview guards, strict returned identity/format, PCM counts, fresh-process/environment isolation, rotating order, partial/oversized frames, output preservation, source-change invalidation, safe network categories and explicit one-call selection without automatic retries.

## Twenty fresh-process observations

The root agent then completed a separate [20-call cold cohort](../scripts/performance/innertube/results/2026-10-19-visionos-v3-cold-20.json): **20/20 eligible**, without automatic retries. Every call used a new Node process and task directories, anonymous service-worker context, exact returned source ID and expected metadata, itag 251 Opus/WebM, and 240,000 decoded stereo frames at 48 kHz (five seconds). Cleanup completed for every call. The raw report is retained byte-for-byte, SHA-256 `27a622ff474f5de9eb54c20997994b1adf439310793b32cb65fdd2f309ee68c0`.

| Cohort/source | Calls | Median first PCM | Nearest-rank p95 | Worst first PCM |
| --- | --- | --- | --- | --- |
| All three sources | 20 | 920.138 ms | 1,427.560 ms | 2,351.132 ms |
| Matthew | 7 | 928.546 ms | 1,367.198 ms | 1,367.198 ms |
| Poso lyric upload | 7 | 842.734 ms | 2,351.132 ms | 2,351.132 ms |
| Timmy Topic candidate | 6 | 894.477 ms | 1,427.560 ms | 1,427.560 ms |

The median averages the middle two values when the sample count is even. P95 uses sorted rank `ceil(0.95*n)`, without interpolation: rank 19 of 20 overall. With only six or seven calls per source, its per-source p95 is the observed maximum. These are descriptive statistics of this short cohort, not population tail guarantees. All 20 observations were below three seconds on this host; no broader startup guarantee follows.

`firstPcmFromCommandMs` starts at probe function entry after JSON parsing and ends at first PCM. It includes context lookup, player request, format validation and decoder startup, and **excludes process launch, module import and JSON parsing**. The driver's 29,489.131 ms total includes serial process launch, excerpt completion and cleanup. `launchToResponseMs` ends after the excerpt has decoded; neither it nor `launchToCleanupMs` measures launch-inclusive time to first PCM. Fresh helpers also do not reset shared OS DNS/TLS/CDN caches.

Resource accounting is complete for all 20 calls. Per-call total measured CPU had median **343.508 ms**, nearest-rank p95 **473.305 ms**, and worst **519.954 ms**. Each total sums Node CPU from probe entry, FFmpeg child CPU and the Python measurement-wrapper CPU; it excludes Node startup/import before probe entry, the Python driver, provider/OS work and native/device output. It is CPU time, not wall time. Median component values are not additive summary totals.

Maximum observed per-process peak RSS was **94.770 MiB** for Node, **20.789 MiB** for the decoder and **19.520 MiB** for the accounting wrapper. Node's lifetime high-water mark includes module startup. These peaks may occur at different times and must not be summed into concurrent whole-pipeline memory; driver/native-application/device memory is outside this measurement.

The [safe derived summary](../scripts/performance/innertube/results/2026-10-19-visionos-v3-cold-20-summary.json) retains exact counts, quantile method, per-source statistics, CPU/RSS statistics and scope, the two report hashes, and the complete source-hash inventory. The repeat and cold report inventories are identical, including the probe, driver, core decoder, budget/measurement utilities, source fixtures, corpus manifest, Node and FFmpeg. Both report `status:completed`, rather than source-change invalidation. Earlier durability used its own retained source inventory and remains a distinct cohort.

The cold sample does not erase the prior player-request failure or establish 100% reliability. The three source IDs, one host/network and one short time window do not establish acoustic/catalog equivalence, native integration behavior, device startup, a real-time soak or release readiness. The fast full-duration decodes were not real-time playback. A future native integration still needs a narrow typed fallback boundary. A same-deadline idempotent player-POST connection retry could be a separately controlled experiment; no such retry or production promotion is implemented here.

## Native Opus decoder-route observations

A later native pipeline diagnostic compares the legacy native-decoder probe followed by FFmpeg fallback with a direct FFmpeg route for the selected Opus/WebM source. The harness resolves one source once per pair and uses that same URL and itag 251 for both arms, with an independent access validation before **each** arm. It retains no signed URL. The shared resolver timing is kept separate from arm timing; these experiments do not compare complete startup, catalog resolution, mixer/device output or audible playback.

The [initial three-pair log](../scripts/performance/innertube/results/2026-10-19-native-opus-route-three.log), [safe rows](../scripts/performance/innertube/results/2026-10-19-native-opus-route-three-rows.json) and [paired summary](../scripts/performance/innertube/results/2026-10-19-native-opus-route-three-summary.json) preserve 3/3 eligible pairs and six eligible arms in AB/BA/AB order. Direct-minus-legacy first-PCM arm deltas were −1,319.097 ms for Matthew, −856.341 ms for Poso and −1,361.139 ms for Timmy. Common resolution was separately 651.583, 559.082 and 255.880 ms. These three observations are not pooled into the larger cohort below.

The subsequent [20-pair log with three warmups](../scripts/performance/innertube/results/2026-10-19-native-opus-route-twenty.log), [safe parsed requests](../scripts/performance/innertube/results/2026-10-19-native-opus-route-twenty-rows.json) and [summary](../scripts/performance/innertube/results/2026-10-19-native-opus-route-twenty-summary.json) retain **all 23 requests**. The 20 measured pairs succeeded, but only **two of three warmups** succeeded. The first Matthew warmup returned `PLAYER_HTTP_ERROR` during common player resolution after 305.452 ms, before either arm started. Its HTTP status and underlying cause were not retained. The harness then failed its expected-3/actual-2 warmup assertion and exited **101**: the overall run did **not** pass. There were 22 successful pair requests out of 23 including warmups, 44 eligible arms actually started, and two arms not started after that common failure. This is not a 100% provider-reliability result.

The following statistics use only the 20 measured pairs: seven Matthew, seven Poso and six Timmy, with ten AB and ten BA orders. P95 is nearest-rank 19/20 without interpolation; warmups are excluded from timing quantiles while remaining included in the failure accounting above.

| Measured metric | Legacy native probe median / p95 / worst | Direct FFmpeg median / p95 / worst |
| --- | --- | --- |
| Arm start → first PCM, including access | 1,838.036 / 2,198.385 / 2,567.101 ms | 412.434 / 577.289 / 1,339.080 ms |
| Independent access validation | 194.786 / 405.253 / 460.142 ms | 210.723 / 298.641 / 779.724 ms |
| After access → first PCM | 1,643.354 / 1,940.466 / 2,106.960 ms | 212.103 / 322.594 / 559.356 ms |

The paired **direct-minus-legacy arm** delta had median **−1,374.190 ms**, nearest-rank p95 **−1,089.681 ms**, and range **−2,352.303 to −859.305 ms**. A larger signed delta is less favorable for the direct route; this p95 is not an upper quantile of positive savings. All 20 observed paired differences favored direct FFmpeg. Each legacy arm reported a failed native decoder setup before FFmpeg; the direct arms skipped that attempt. Access and other sequential I/O differences remain, so the deltas should not be interpreted as an isolated CPU-cost measurement.

Common resolution, performed once and shared by both arms, had median **220.671 ms**, p95 **505.543 ms** and worst **702.693 ms** in the measured cohort. It is not added to either sequential arm to synthesize a full-startup comparison. The HTTP client was reused, and the three source IDs, short run and sequential orders limit broader conclusions.

All 40 measured arms decoded exactly 240,000 stereo frames at 48 kHz, with equal reported RMS within each pair; the four successful warmup arms did as well. Equal frame count and RMS do not establish identical waveforms, recording equivalence or audibility. In particular, Poso's first five seconds were near-silent in **both** arms, RMS approximately **6.80083×10⁻⁶**. This must not be called audible playback or used alone to infer a wrong recording. No physical device test, real-time soak, full-recording decode or CPU/RSS comparison is present in this paired log; zero FFmpeg byte counters do not mean zero network transfer.

The binary SHA-256 supplied by the root agent is `f93ed01073ca91b4f059c65462266159b62df2f3ed1ce1d5cacaa9a94d312ea5`. Implementation bodies are attributed to commit `96fc8c9`; after compilation, only `lib.rs` declaration ordering/formatting changed. The binary is therefore not claimed to represent a byte-identical snapshot of the later complete working tree. The earlier default-suite result reported by the root agent was 1,285 passed, zero failed and 30 ignored; that separate result does not turn the warmup-assertion failure into a passing live test. The unchanged 20-pair log SHA-256 is `0f1001bf9960f389ed8e8296abc0ed2301de5e7a1f1643b60a6d852d6c0aa1bb`; the summaries retain build provenance and the earlier three-pair log hash as well. No promotion follows from these observations.
