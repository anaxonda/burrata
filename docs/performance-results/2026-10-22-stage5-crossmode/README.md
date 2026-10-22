# Stage 5 cross-mode diagnostic smoke sample

On 2026-10-22, the frozen `playback_reliability` diagnostic ran paired cold/warm attempts for the Bacc and Vibe saved public fixtures at Exact, Balanced, and Flexible. Each mode/case is one sample; this is a trace and integration smoke sample, not a latency qualification. Six paired invocations produced twelve measured attempts, with no warmups.

## Frozen inputs

- Diagnostic binary SHA-256: `937e4cbbcc055a477a9af7d141d4d12d38eaf906e6a424a694791fbdc83df073`
- Diagnostic source SHA-256: `02abc81d239dc70f713a61d06f170964cad6bf06bc05ee0432bbc680b00e85fe`
- Source revision: `9d5a4194111b023ba9252d85039aeb50e7b6fcfa`
- Manifest SHA-256: `7202ffdf4fc69a9e927dc1b7c1cad611b7b8a0cd68608671849d4531ee58176a`
- Public settings SHA-256: `b600b7115802cd2fcfa8f9df1d8f32729d0c209ad9b8ad335e88d3bb72d1d41d`
- Tracks: Bacc `6703178b0ec43f2e6f6c021e4f1ee6868516302186f2c71fa98487e0d658afdd`; Vibe `99604787c9f87835f25a543fe4bf71d2e0402770c771bd27b4b42aece7c3a1e4`
- Provider policy stayed fixed: preferred Monochrome; fallback enabled; YouTube then SoundCloud. Matching level was the only setting changed.

## Results

`resolveMs` is the diagnostic’s wall-clock resolver duration. Exact attempts failed with `SOURCE_UNAVAILABLE`; their durations remain in the denominator and are not successful-resolution latency. Balanced and Flexible succeeded on both fixtures and paired to the same resolved YouTube IDs within each fixture. p95 is withheld because each cell has one attempt.

| Fixture | Level | Cold result / resolve ms | Warm result / resolve ms | Cold logical extraction starts | Warm logical extraction starts | Warm mapped direct YouTube |
|---|---|---:|---:|---:|---:|---:|
| bacc-saved | Exact | SOURCE_UNAVAILABLE / 16465.7 | SOURCE_UNAVAILABLE / 2864.1 | 2 | 1 | 0 |
| bacc-saved | Balanced | PASS / 9504.5 | PASS / 550.2 | 1 | 0 | 1 |
| bacc-saved | Flexible | PASS / 10917.8 | PASS / 1047.0 | 1 | 0 | 1 |
| vibe-saved | Exact | SOURCE_UNAVAILABLE / 17855.7 | SOURCE_UNAVAILABLE / 2202.8 | 2 | 1 | 0 |
| vibe-saved | Balanced | PASS / 8984.6 | PASS / 661.9 | 1 | 0 | 1 |
| vibe-saved | Flexible | PASS / 8752.4 | PASS / 618.2 | 1 | 0 | 1 |

## Trace observations

Counts below are allowlisted logical trace events correlated to the attempt’s native operation ID. They are not physical HTTP request counts. `extract_and_validate_start` marks resolver work; `extraction_start` marks logical extraction events. The resolver-budget counter is an observed emitted snapshot and can be zero or absent even when extraction events occurred.

| Level | Cold search starts / candidate gates / extract-and-validate starts / extraction starts | Warm search starts / candidate gates / extract-and-validate starts / extraction starts |
|---|---:|---:|
| Exact | 6 / 6 / 6 / 4 | 6 / 6 / 6 / 2 |
| Balanced | 4 / 4 / 2 / 2 | 2 / 2 / 0 / 0 |
| Flexible | 4 / 4 / 2 / 2 | 2 / 2 / 0 / 0 |

The two Balanced and Flexible warm attempts each emitted `mapping_resolve_start` and `mapped_direct_youtube_start`, with zero logical `extraction_start` events. Exact cold attempts emitted four logical extraction starts across the two fixtures; both Exact cold and warm ultimately failed. No cancellation event was observed. These traces do not exercise a deadline path.

## Batch and limits

The harness cannot run a true two-track batch: the diagnostic CLI accepts one `--track` per process, and the coordinator launches one process per case/level. The “two-track” manifest name only batches the invocations sequentially; it does not share a resolver queue or playback batch. We therefore did not claim batch behavior or invent batch metrics. The sample uses the existing public fixtures and settings only; raw stderr/report/settings files remain in the private `.cache/matching-v016-crossmode-smoke-20261022/` directory.

The sanitized per-attempt aggregate is archived alongside this report. It retains failures, phase counts, native-ID correlation diagnostics, observed extraction-counter snapshots, and the one-sample timing values. No request-total, cancellation/deadline, production-performance, or p95 claim is made.
