# Two-track Balanced resolver batch — sandbox execution invalid

**Do not use these failed attempts as performance results.** The coordinator was run under the default sandboxed command context, without `require_escalated`. All 80 measured attempts returned `SOURCE_UNAVAILABLE`; safe raw reports show `stage=resolver` and `resolved=false`, but no DNS, connection-denied, or HTTP-status detail is present in the captured phase logs. A single follow-up host-network smoke using the same binary, settings, and Bacc fixture succeeded cold and warm. This strongly indicates that the first cohort was blocked by its execution context; it does not establish a provider outage. Its timing values are retained for audit only and are excluded from performance conclusions.

This bounded resolver-only batch ran the saved Bacc and Vibe With Me catalog fixtures under the same public settings, using the frozen `qualification-alias-cache-bpm-v3` diagnostic binary. The coordinator processed the two cases serially; each case contained 20 measured cold/warm pairs and three warmups per scenario. Cold and warm attempts were paired within each track and used disposable stores. This is not an interleaved cross-track-per-repetition schedule.

The sandbox cohort itself did **not** qualify: all 40 measured attempts for each track ended as `SOURCE_UNAVAILABLE` (80/80 failures total). The table below preserves those failed-run timings, but they are not valid cold/warm performance measurements. The paired source-identity difference count of zero is vacuous because no attempts resolved a source. This run does not benchmark the current/new download route or the production app artifact `ed056432…`.

**Quarantined failed-attempt timing values (audit only):**

| Fixture | Cold failed attempts | Cold p50 / p95 (ms) | Warm failed attempts | Warm p50 / p95 (ms) |
|---|---:|---:|---:|---:|
| Bacc Of The Bach | 20/20 | 3495.9 / 3906.1 | 20/20 | 3479.8 / 4018.1 |
| Vibe With Me | 20/20 | 3911.1 / 4145.6 | 20/20 | 3915.9 / 4113.0 |

The trace parser correlated every measured attempt to its native operation ID. Per scenario and fixture it observed 20 resolution starts/ends, 60 alias-cache lookup ends, 60 provider/search starts and ends, 20 Bandcamp fallback starts/ends, and 20 alias-lookup-skipped events. No `extraction_start` events were observed. The resolver extraction-budget counter had 20 zero-valued snapshots per scenario. These are instrumented logical event counts and counter snapshots, not physical extraction or HTTP request totals.

The settings fixture preferred Monochrome with fallback enabled to YouTube and SoundCloud, with the same policy for both cases. The follow-up host-network smoke resolved the same YouTube recording ID in both scenarios; see the separate smoke artifact. Binary SHA-256: `7a95ec034871cfbd205ed022fd634e7ae8c97d71e43f0a985fbf49eb25726f6a`; diagnostic example source SHA-256 reported by the binary: `02abc81d239dc70f713a61d06f170964cad6bf06bc05ee0432bbc680b00e85fe`; input manifest SHA-256: `7202ffdf4fc69a9e927dc1b7c1cad611b7b8a0cd68608671849d4531ee58176a`; repository revision: `9d5a4194111b023ba9252d85039aeb50e7b6fcfa`.

The coordinator's sanitized aggregate is [available as JSON](bacc-vibe-balanced-v3-batch.json). Raw reports and process logs remain in the ignored `.cache/matching-stage5-two-track-balanced-cohort-20261021/` directory with owner-only permissions. The smoke aggregate is [available separately](bacc-v3-hostnet-smoke.md).

The current coordinator accepts a multi-case manifest but completes each case before starting the next. If a synchronized Bacc/Vibe schedule is required, a minimal harness-only change can schedule one diagnostic pair per case per repetition (plus explicit warmup pairs), then merge the sanitized rows; no production resolver or Cargo changes are needed.
