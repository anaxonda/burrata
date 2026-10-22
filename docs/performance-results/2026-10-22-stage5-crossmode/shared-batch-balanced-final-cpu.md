# Final CPU-instrumented Balanced shared-process cohort

This is a new 20-pair cold/warm run using the final CPU-instrumented diagnostic binary. The saved public Bacc and Vibe fixtures were resolved sequentially in one process and one disposable Store per two-entry batch. All 20 cold and 20 warm measured batches passed, all 40 entries per scenario succeeded, and the paired cold/warm identities matched. Three warmup pairs are excluded. This bounded result describes two fixtures under the captured Balanced policy; it is not a population estimate or a GUI queue measurement.

## Captured inputs

- Diagnostic binary SHA-256: `8836c551b620a7ba4ba7b487f48d23ca93436758c748ffe543da310a02dcfc36`
- Diagnostic example source SHA-256: `0c61b91f35f6c12518e3dd8e72e89c4c7d429fce9c8cf1108860c5cd31e79ee1`
- Base Git revision: `129a25bd087ec2bc89cf6129e5d24aaa8f014fe3`; the worktree was dirty at capture. The recorded tracked-diff SHA-256 is `67e75e6a558f24715d4ed62566cda720c2c754511f8aa1dd5fe5c1eaed86c8d1`, and the untracked diagnostic meter source SHA-256 is `62bb68da280772b1d09e39a54651fbf4d1a4b0f61a4db10a2aa1710b0e26076e`.
- Full source/dist manifest SHA-256: `a3b7015f3e6542cd95ac1d030f6fc113562fb79b1ffcc50500bde8803ecc29eb`; file digests are in the build evidence at `.cache/qualification-v016-final-cpu-20261022/source-manifest.json`.
- Manifest SHA-256: `de0cadff3f0c61eeca172c086a4127c184932b028405eb15fb0141b61952a95c`; public settings SHA-256: `b600b7115802cd2fcfa8f9df1d8f32729d0c209ad9b8ad335e88d3bb72d1d41d`.
- Track fixture SHA-256: Bacc `6703178b0ec43f2e6f6c021e4f1ee6868516302186f2c71fa98487e0d658afdd`; Vibe `99604787c9f87835f25a543fe4bf71d2e0402770c771bd27b4b42aece7c3a1e4`.
- Policy: Balanced; preferred Monochrome; fallback enabled to YouTube and SoundCloud; 20-second per-entry startup budget. Execution used host networking with the public fixtures and settings only.
- Cohort command: `python3 scripts/performance/matching_cohort.py --binary .cache/qualification-v016-final-cpu-20261022/bin/playback_reliability --manifest .cache/matching-stage5-shared-batch-smoke-20261022/manifest.json --outputdir .cache/matching-stage5-final-cpu-20261022/balanced-cohort --reps 20 --warmups 3 --levels balanced --trace`.

The binary was built from the dirty worktree based on the stated revision. The binary hash and source/diff manifest identify the captured build; the base revision by itself does not. Unlike earlier pre-fix evidence, this binary was built after the acquisition asset-selection correction. The test remains a diagnostic sample and does not claim broad release qualification.

## Results

All times are milliseconds, reported as p50 / p95 over 20 measured samples. Failures remain in the denominator; all samples in this cohort succeeded.

| Scope | Cold p50 / p95 | Warm p50 / p95 | Successes |
|---|---:|---:|---:|
| Two-entry batch wall time | 17,116 / 18,864 | 1,296 / 2,841 | 20/20 batches each |
| Bacc resolver | 9,270 / 10,882 | 396 / 1,000 | 20/20 entries each |
| Vibe resolver | 7,327 / 7,995 | 340 / 870 | 20/20 entries each |
| Bacc first PCM from request | 9,424 / 10,991 | 623 / 1,399 | 20/20 successful entries each |
| Vibe first PCM from request | 7,575 / 8,905 | 537 / 1,246 | 20/20 successful entries each |
| Batch helper CPU | 924 / 1,014 | 246 / 276 | 20 samples each |
| Reaped child CPU | 7,474 / 8,021 | 168 / 225 | 20 samples each |

Batch thread CPU from the new Linux `CLOCK_THREAD_CPUTIME_ID` instrumentation is distinct from process CPU and wall time. Matching preparation elapsed wall-time p50/p95 was 182.62 / 239.99 ms cold and 52.89 / 65.17 ms warm; corresponding calling-thread CPU was 180.26 / 230.94 ms cold and 52.26 / 64.18 ms warm. Matching assessment elapsed wall-time p50/p95 was 1.270 / 1.661 ms cold and 0.421 / 0.535 ms warm; corresponding thread CPU was 1.302 / 1.700 ms cold and 0.428 / 0.534 ms warm. All 20 batch windows had CPU observations for both stages; the per-window median observation counts were 143 cold and 52 warm. These stage-local values cover only observed synchronous current-thread intervals, not total process CPU or async worker CPU. Instrumentation overhead and slightly different timestamp boundaries are included; CPU/wall ratios here are not utilization estimates.

The logical trace correlated all 2,384 allowlisted events. The 40 cold entries emitted 40 logical extraction starts and ends; the 40 warm entries emitted zero logical extraction starts. Resolver extraction-budget snapshots are a separate counter and must not be interpreted as physical extraction or HTTP counts. Instrumented Rust send attempts were 23 p50 / 27 p95 per cold batch and 8 / 8 warm; successful yt-dlp starts were 2 / 2 cold and 0 / 0 warm. Physical outbound request count remains null because redirects, yt-dlp internal requests/retries, and uninstrumented modules are not counted.

The highest observed RSS values were approximately 86.4 MB for the helper and 358.7 MB for reaped children. They are process lifetime high-water readings, not per-batch/per-entry memory and not additive. Batch CPU is cumulative across the serial two-track window. The aggregate records the instrumented-site coverage and exclusions.

The final-binary one-pair crossmode smoke is in [`shared-batch-crossmode-final-cpu-smoke.json`](shared-batch-crossmode-final-cpu-smoke.json). Balanced and Flexible passed all four cold/warm entries each. Exact retained four failures out of four attempts, all `SOURCE_UNAVAILABLE` with no resolved provider identity; Exact latency qualification is therefore unavailable.

The sanitized cohort aggregate is [`shared-batch-balanced-final-cpu.json`](shared-batch-balanced-final-cpu.json). Private process logs and raw diagnostic reports remain under `.cache/matching-stage5-final-cpu-20261022/`.
