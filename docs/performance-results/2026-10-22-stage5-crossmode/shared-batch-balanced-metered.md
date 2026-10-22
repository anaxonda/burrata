# Shared-process Bacc + Vibe cold/warm batch cohort

The diagnostic ran the saved public Bacc and Vibe fixtures sequentially in one process and one disposable Store for each two-entry batch. The 20 measured cold/warm pairs completed with 40/40 successful entries in each scenario and no source-identity changes. Three warmup pairs were excluded from the timing summaries. This is a two-fixture serial batch cohort, not an estimate for other catalogs, concurrent playback, or the UI queue.

## Frozen inputs

- Diagnostic binary SHA-256: `8af63b4f1ccdc919826735f3fc3eec3f6eb21e487ea91a5336d7e412e057fc0b`
- Diagnostic source SHA-256: `1dbf667bba9c4d2f73e35d652d80ba6dc8a67e0291c16c1e8a66b30853b457f4`
- Base Git revision: `129a25bd087ec2bc89cf6129e5d24aaa8f014fe3`
- Manifest SHA-256: `de0cadff3f0c61eeca172c086a4127c184932b028405eb15fb0141b61952a95c`
- Settings SHA-256: `b600b7115802cd2fcfa8f9df1d8f32729d0c209ad9b8ad335e88d3bb72d1d41d`
- Track fixture SHA-256: Bacc `6703178b0ec43f2e6f6c021e4f1ee6868516302186f2c71fa98487e0d658afdd`; Vibe `99604787c9f87835f25a543fe4bf71d2e0402770c771bd27b4b42aece7c3a1e4`
- Policy: Balanced; preferred Monochrome; fallback enabled to YouTube and SoundCloud; 20-second per-entry startup budget.
- Execution used host networking (`exec_command` with `require_escalated`).
- Coordinator command: `python3 scripts/performance/matching_cohort.py --binary .cache/qualification-v016-final-metrics-playlist-20261022/bin/playback_reliability --manifest .cache/matching-stage5-shared-batch-smoke-20261022/manifest.json --outputdir .cache/matching-stage5-shared-batch-smoke-20261022/metered-balanced-cohort --reps 20 --warmups 3 --levels balanced --trace`.

The binary was built from a dirty worktree based on the listed Git revision. It includes uncommitted diagnostic and provider instrumentation; the binary and diagnostic-example hashes above identify the captured artifacts, but no complete source snapshot was recorded at build time. This report therefore does not claim that the base revision alone reproduces the binary. The acquisition asset-selection fix landed after this binary was built, so these measurements describe the pre-fix build and do not qualify the newer final source.

## Results

All values are milliseconds unless specified. Batch p95 uses 20 measured batch samples. Entry p95 uses 20 samples per fixture. Cold and warm phases share the same two-item order in every pair.

| Scope | Cold p50 / p95 | Warm p50 / p95 | Successes |
|---|---:|---:|---:|
| Two-entry batch wall time | 18,072 / 19,784 | 1,583 / 2,670 | 20/20 batches each |
| Bacc resolver | 9,114 / 10,172 | 574 / 1,148 | 20/20 each |
| Vibe resolver | 8,045 / 9,734 | 405 / 800 | 20/20 each |
| Bacc first PCM from request | 9,291 / 10,292 | 793 / 1,541 | 20/20 each |
| Vibe first PCM from request | 8,385 / 10,072 | 605 / 1,171 | 20/20 each |
| Batch helper CPU | 871 / 972 | 260 / 301 | 20 samples each |
| Reaped child CPU | 7,372 / 7,870 | 126 / 243 | 20 samples each |

Per-attempt resolver time is distinct from time to first PCM. The child CPU measure covers reaped helper/extractor/FFmpeg children in the batch window; persistent child work not yet reaped is excluded. Local matching preparation and assessment timers are synchronous elapsed-time sums, not CPU time.

This archived run predates the diagnostic's per-thread matching-stage CPU fields, so matching CPU time is unmeasured here. Future diagnostic captures will report preparation and assessment thread CPU nanoseconds only when `CLOCK_THREAD_CPUTIME_ID` observations are available, alongside observation counts; unsupported or unobserved windows remain null rather than zero. Those stage-local values do not represent total process CPU or async-worker CPU.

## Trace and instrumented counters

Native operation IDs correlated all 2,366 allowlisted trace events. Across the 40 cold entries, traces recorded 80 search starts, 40 logical extraction starts, 40 extraction ends, and 80 preview-audio lookup starts and ends. Across 40 warm entries, they recorded 40 mapping-resolution and 40 mapped-direct-YouTube starts and ends, with zero logical extraction starts. Phase p50/p95 includes alias-cache lookup 4.62/12.15 ms cold and 2.98/5.35 ms warm; catalog alias preparation 338/759 ms cold; canonical-search warmup 1,282/1,911 ms cold; extraction end 5,408/7,867 ms cold; and mapped-direct-YouTube end 387/953 ms warm. These are elapsed phase observations, not additive when work overlaps.

Instrumented send-invocation counts per batch were 23 p50 / 25 p95 cold and 8 / 8 warm. Successful yt-dlp child starts were 2 / 2 cold and 0 / 0 warm. Synchronous matching preparation elapsed sums were 175.15 / 204.52 ms cold and 51.81 / 62.59 ms warm; assessment sums were 1.16 / 1.41 ms cold and 0.385 / 0.463 ms warm.

These process-window counters cover only the Rust send sites listed in the aggregate's `instrumentedMetricsCoverage`; overlapping instrumented work in the process is included. Automatic redirects, yt-dlp's internal requests, retries and redirects, and uninstrumented modules remain opaque. They are not physical outbound HTTP request totals; that field remains null. Extraction event counts are logical resolver events, separate from successful child starts.

Batch-end RSS observations are lifetime high-water marks, not per-batch or per-entry memory. The highest observed values were about 87.4 MB for the helper and 360.2 MB for reaped children; they must not be treated as simultaneous or additive batch memory.

The diagnostic smoke across all modes is archived in [`shared-batch-crossmode-smoke.json`](shared-batch-crossmode-smoke.json): Balanced and Flexible passed all four cold/warm entries; Exact retained four failures (one timeout and three source-unavailable outcomes across the two tracks), so this smoke does not provide Exact latency qualification.

This closes the bounded single and shared-process batch timing sample for the two fixtures at Balanced, and provides a one-pair trace smoke across all modes. It does not measure a physical HTTP total, exclusive per-entry process RSS, concurrent queue processing, or a separately timed access-validation phase; those remain explicit instrumentation limits rather than inferred values.

The raw reports and process logs remain owner-readable in `.cache/matching-stage5-shared-batch-smoke-20261022/`. The sanitized batch aggregate is [`shared-batch-balanced-metered.json`](shared-batch-balanced-metered.json).
