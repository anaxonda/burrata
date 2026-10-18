# Stale-projection index: asset-backed 50k exploratory run

The isolated development candidate uses a revision-specific partial index to discover stale recording projections without grouping all ready assets. Its immutable executable and source hashes, exact command and terminal result are in [run-manifest.json](run-manifest.json). Focused storage tests pass 74 cases (3 ignored); the separate older-baseline host suite passes 719 (16 ignored). This is not current-main verification.

The Store-only fixture has 50,000 listings with varied BPM and distinct zero-byte assets, 200-row pages, three warmups and three measured samples per mode. There is no audio decoding.

| Ready page | Indexed median / worst (ms) | Matched no-index median (ms) |
| --- | --- | --- |
| First ascending | 378.05 / 380.96 | 933.94 |
| Later ascending | 319.98 / 369.23 | 927.26 |
| First descending | 359.25 / 373.50 | 886.39 |
| Later descending | 318.59 / 363.22 | 911.14 |

Pending first/later checks take median 228.53/216.66 ms and correctly return pending with no page rows. Full backfill takes 87.89 seconds in 100 batches of 500 rows; worst benchmark batch is 984.33 ms. Runtime batches use 250 rows. BPM-index population takes 1.067 seconds. Index maintenance mode was disabled for this run.

Indexed ready-stage profiles put stale pending checks at approximately 0.15–0.24 ms, discovery at 0.3–0.5 ms, hydration at 89–124 ms, first-page global count/availability at 226–277 ms, and page SQL at 2–5 ms. See [indexed-stage-aggregation.json](indexed-stage-aggregation.json) and [summary.json](summary.json).

The complete final harness summary is retained, but stdout was not redirected during execution: individual emitted profile lines are unavailable. No rerun was performed to recreate them. Three observations do not qualify the 20-sample gate, and these measurements do not qualify playback startup, complete application resources, release builds or other platforms. Current-main integration and qualification remain required before promotion.
