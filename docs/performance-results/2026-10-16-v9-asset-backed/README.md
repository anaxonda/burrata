# Persisted availability: asset-backed reads and migration checkpoint

## Asset-backed first-page performance

These were actual Store microbenchmarks on the v9 prototype with sparse catalog metadata and distinct zero-byte local assets supplying BPM and key. Each mode had three warmups and three measured samples. The page size was 200, with BPM sorting. GUI, end-to-end work, and CPU time were excluded. The sample count is exploratory and does not qualify a p95 target; with three samples, the reported p95 is only the sample maximum.

| Library size | Ready first page median / max (ms) | Warm ready median / max (ms) | Legacy page median / max (ms) | Full cold backfill median / max (ms) |
| ---: | ---: | ---: | ---: | ---: |
| 1,000 | 609.09 / 627.94 | 610.32 / 624.77 | 607.15 / 652.61 | 2.15 / 2.16 |
| 5,000 | 2,589.95 / 2,654.78 | 2,674.26 / 2,683.45 | 2,560.04 / 2,560.47 | 11.21 / 14.93 |

Both ready asset-backed reads exceed the 500 ms target, and their timings are close to or slower than the legacy page. The 5,000-row instrumented profile attributes most time to asset decode/hydration and asset mask materialization. Its 18 emitted stage observations span warmup and measured calls; their medians include instrumentation overhead and are descriptive only. The stage timings overlap and must not be summed. This identifies dynamic asset-backed hydration as the measured bottleneck; it does not support promotion of the current candidate.

## Preserved-data migration

The preserved snapshot was schema v7, despite the older backup directory label. A disposable SQLite-backup copy exercised the v7→v9 migration through the candidate `Store::open`; a second copy was advanced to schema v8 in `/tmp` to test v8→v9 with both membership values present. Across all 21 original tables, every original-column row multiset matched after migration for 4,948 catalog listings. The v7 migration supplied its historical saved-membership default; the v8 copy retained its saved and cache-only membership values. All new availability masks remained pending. SQLite `quick_check` returned `ok`, and `foreign_key_check` found zero errors.

`Store::open` also reconciles nonterminal download jobs at startup. The preserved snapshot contained only terminal jobs (10 cancelled, 15 completed, 19 failed), and those rows remained unchanged. No row data, per-table hashes, account data, or private database locations are included here.

## Native and GUI evidence

The separate v9 prototype native suite passed 705 tests with 15 ignored. The main native checkpoint remains 693 passing tests; the prototype count is a different build and must not be presented as main-source verification.

The E5 GUI passed its verified-launcher mounted handshake. A live geometry inspection reported a 1,914×341 viewport, an 86 px footer, and no detected layout findings. The populated library had zero visible rows because the header, filters and player filled this short viewport. A subsequent compact-height frontend change was visually checked on the same owned main-window rectangle: one track row and the entire player were visible. This remains a single viewport check. YouTube account confirmation is still open: the UI requests sign-in on music.youtube.com, and no account was confirmed.

The archived summary contains aggregate values only. These results are development evidence, not release or whole-application performance qualification.
