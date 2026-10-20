# Direct startup: additional sources and verified mappings

These are internal, opt-in playback research captures. No release or default enablement follows from them. The ordinary user window stays on its existing profile with the runtime experiment off.

## Frozen explicit-source measurements

The test executable was built at `e06bf2a`, SHA-256 `2f21fcfbf197b327680b2094b4933cd25eb47e61e7076327f218921f30512621`. Production resolution, uncached access validation and five seconds of native decoder PCM run in a fresh test process for each request. Timing starts at resolution, excludes process launch, catalog search, GUI, mixer and device output. Processes do not reset OS DNS/TLS or provider state. No build ran during these measurements.

- [Pengus matrix](pengus-matrix/report.json): three exploratory successes, then **20/20 measured successes** on the captured `Pengus - Victory Lap` video. First PCM median **1,031.025 ms**, nearest-rank p95 **1,309.924 ms**, worst **1,735.486 ms**. Resolution median 771.787 ms. Aggregate waited-process CPU median 0.485 seconds, worst 0.720 seconds; maximum process RSS median 84,710 KiB. These resource measures do not describe startup peak or simultaneous process-tree memory. Zero helper-preparation/extraction trace events occurred.
- [Six-source exploration](six-source-exploration/report.json): **18/18 successes**, three exploratory repetitions each on captured Pengus, Bathi Ndiyabanda, Matthew, Poso, another Matthew upload and BAD TIMES IDs. Every request decoded 240,000 frames. This provides transport/decoder variety, not statistical qualification per source or proof that different uploads equal a requested catalog edition.

The two drivers and original logs are retained. Source-fixture paths/hashes are in their reports. The manifest hashes archived files byte for byte; `.log` files retain their original final newlines.

## Verified mapping integration

The catalog-only implementation at `e06bf2a` passed [1,312 native tests, zero failures, 33 ignored](catalog-only-feature-suite.log). Its first [two-pass live test failed](mapped-catalog-only-first-failed.log): normal search/extraction successfully resolved the requested catalog recording in 15,452 ms, but the resulting mapping did not contain the eligible catalog-only v2 proof. No direct second pass or PCM was claimed. The original failed capture is retained.

The later source change at `80c6af7` broadens v2 anchors to all three existing accepted recording proof kinds, retaining their original kind. It requires fresh exact video/channel IDs, title and duration from the successful normal identity/access path; incomplete anchors retain v1 behavior. A later direct response must match those anchored facts before uncached access. Direct success never refreshes the mapping's original expiry, and no artist credits or verified-channel status are fabricated from player data. At `80c6af7`, the [generalized native suite](generic-feature-suite.log) passed **1,313 tests, zero failed, 33 ignored** in 23.02 seconds. Its [first two-pass live check](mapped-generic-live-first.log) succeeded: normal catalog mapping creation took **15,931 ms**; reuse resolved directly in **741 ms**, reached first PCM in **1,003.699 ms**, decoded 240,000 frames and left expiry/update timestamps unchanged. The test binary SHA-256 is `097c643ab66dc9bedfcaf5073b63081720eb5c0d9b864036a20599b19204bd6a`. This is one exploratory catalog mapping, with decoder PCM rather than mixer/device output; it is not a statistical catalog result.

Known-ID startup results above do not remove the several-second search cost for first-time catalog recordings. Code inspection also identified a possible first-time direct arm: a fresh source title containing complete performer credits can sometimes pass the existing identity verifier without structured channel metadata. This remains unimplemented and unmeasured; title-only or incomplete-credit results must retain fresh extraction.

## Linux diagnostic checkpoint

[Run 39361232840](https://github.com/anaxonda/burrata/actions/runs/39361232840), source `d4f4fc1`, passed macOS and Windows but failed two Linux Opus fixtures (1,298 passed, two failed, 30 ignored). The [retained log](ci-39361232840-failed.log) shows Ubuntu FFmpeg 4.4.2 itself returns 95,376 float samples (47,688 stereo frames) for the same one-second command used by the app. This isolates the discrepancy from the app pipe reader; newer local/bundled FFmpeg yields 48,000 frames. The helper-failure fixture passed. The subsequent [Ubuntu probe](ffmpeg44-timestamp-probe.json) at `6cf533c` reports default output 95,376 float samples and timestamp-normalized output 96,000; the full [job log](ci-39362685380-linux-probe.log) is preserved. This supports testing `asetpts=PTS-STARTPTS` for finite ranges. No duration assertion has been weakened and no production decoder change follows from this diagnostic alone.


## Repeated mapped starts and retained source IDs

The first attempted 23-trial action at `c23399f` [stopped during bootstrap](mapped-repeat-bootstrap-channel-id-failed.log), before any direct trial: a valid ordinary recording was resolved, but its source channel ID had been discarded because the uploader was unverified. Its v1 mapping remained usable through yt-dlp. This failure is preserved and not counted as a successful matrix.

At `b851e3e`, extraction retains a valid channel ID as an equality-only fact when richer verified/Topic metadata is absent. It adds no artist, uploader or verified-badge claim; regressions assert title-only candidates still fail the strong recording gates. The finite-window FFmpeg timestamp correction is also present. The [feature suite](id-retention-feature-suite.log) passed **1,314 tests, zero failures, 33 ignored** in 23.12 seconds.

The subsequent [mapped repetition report](mapped-repeat-report.json) has **3/3 exploratory plus 20/20 measured successful direct requests** after one genuine fresh mapping bootstrap. Every trial decoded 240,000 frames and preserved the original expiry/update timestamps. The first mapping took 15.929s and used the existing `verified_extractor` proof kind. Requests share one process/anonymous HTTP client and mapping, with fresh player responses; these differ from the fresh-process explicit-ID measurements. Test binary SHA-256: `fed1924db8263e2a1e718bbd4faaafa0f6e5e71136d4e960ddd818a421c8f41a`.

Measured resolution median **449.909 ms**, p95 **626.381 ms**, worst **962.852 ms**. Decoder first PCM median **230.347 ms**, p95 **344.612 ms**. The per-trial sum of those phase timers has median **672.727 ms**, p95 **877.228 ms**, worst **1,307.464 ms**; it omits synchronous adapter/assertion work between phases and is not instrumented command-to-mixer latency. No physical output was measured, no build ran during the matrix, and no per-trial retries occurred. The bootstrap and earlier failed action remain in the denominator narrative rather than being replaced by this successful action.

[CI run 39364424700](https://github.com/anaxonda/burrata/actions/runs/39364424700) targets `b851e3e`; it is pending at this checkpoint. The earlier diagnostic workflow is terminal and failed, so no all-platform result is claimed for the fixes.

## First-time Music search feasibility

A separate two-query anonymous protocol probe reproduces the native WEB_REMIX request. The [sandbox attempt](music-search-sandbox-failed.json) failed before HTTP; the unchanged request with [host network access](music-search-host.json) returned HTTP 200 and ten parsed items for both queries. The scripts and both reports are retained. These ran during a Rust build, so their timings are feasibility observations, not startup qualification.

`John Denver Matthew` returned the expected title-only Matthew/John Denver/228s item, alongside live versions and a different artist's same-title item. `Pengus Victory Lap` did not include the captured correct source in the first ten parsed rows; those rows were unrelated. No continuation or exhaustive search was attempted. Native Music search can nominate IDs, but those search facts must not be substituted for fresh artist/channel proof. Earlier source inspection found this native search already exists after general search; moving/overlapping a bounded branch and qualifying its final identity proof remain future work.
