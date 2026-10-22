# Saved Bacc Flexible paired cohort

This public-fixture diagnostic used the frozen **evaluator3** binary
(`7d96f877b316bc1a4f0e8eeb1d8c32c8a7c0dedb0a73b46f93186c3451588432`), with
diagnostic source SHA256
`02abc81d239dc70f713a61d06f170964cad6bf06bc05ee0432bbc680b00e85fe` and source
revision `9d5a4194111b023ba9252d85039aeb50e7b6fcfa`. Evaluator v4 is pending;
these results do not qualify v4. The run used the public settings fixture with
Monochrome preferred and YouTube/SoundCloud fallbacks, in an isolated
disposable Store. No normal-profile account or library was read.

Three warmup pairs were excluded, then 20 cold/warm pairs ran sequentially.
Cold creates a new Store/AcquisitionManager and stopped provider helper; warm
immediately reuses the paired context. OS and packaged-tool caches were not
flushed. All 40 measured attempts passed and every attempt resolved to
`youtube:rpCswezRrLs`; no cold/warm source identity changes were observed.

| Resolver measurement | Cold p50 | Cold p95 | Warm p50 | Warm p95 |
|---|---:|---:|---:|---:|
| Resolve | 9999 ms | 11607 ms | 402 ms | 987 ms |
| Request to first decoded PCM | 10201 ms | 11838 ms | 604 ms | 1289 ms |

Each p95 uses 20 observations; all failures would remain in each denominator.
This is one recording on one machine/network, so it describes this run and does
not establish a before/after performance improvement or broad accuracy result.

Trace mode correlated 1,196 allowlisted events to the 40 diagnostic operation
IDs: 660 in cold attempts and 380 in warm attempts. Every event matched directly
by native operation ID; no ID-less shared-lookup fallback or ambiguous join was
needed for this run. Cold attempts each emitted one logical `extraction_start`
event (20 total) and warm attempts emitted none. Cold `extraction_end` duration
was 6124 ms p50 / 7739 ms p95; warm `mapped_direct_youtube_end` was 295 / 908 ms.
Cold catalog alias preparation was 480 / 775 ms, and canonical search warmup
was 2251 / 5312 ms. These phase durations can overlap and are not additive.

The observed resolver extraction-budget counter snapshots were zero in all 20
cold attempts and one in all 20 warm attempts. The cold zero does not mean no
extraction: the same traces record 20 logical `extraction_start` and 20
`extraction_end` events. These are different measurements; neither is an HTTP
request count or a physical-request total. Per-attempt RSS is omitted because
the available child RSS is a lifetime high-water mark.

The sanitized aggregate is [flexible-bacc-traced-cohort.json](flexible-bacc-traced-cohort.json).
Owner-readable raw report and stderr traces remain under
`.cache/matching-stage5-flexible-traced-cohort-20261021/`.
