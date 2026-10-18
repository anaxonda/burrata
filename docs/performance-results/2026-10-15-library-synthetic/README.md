# Synthetic library discovery matrix

Run on 2026-10-15 with Python 3.14.8 on Linux x86_64, using the command in
[`playback-performance.md`](../../playback-performance.md). The [raw JSON](summary.json)
contains all observed summary metrics and workload limitations.

The script seeded an in-memory SQLite database with synthetic JSON metadata,
then compared a Python hydrate-all search loop with SQL filtering and 50-row
paging. Each implementation received three untimed warmups and 20 timed runs
per size. p95 is nearest-rank over those 20 observations. Failures count a
result/count/page mismatch; all four sizes had zero failures.

| Rows | Implementation | Wall median / p95 / worst (s) | Process CPU median / p95 / worst (s) | Hydration reads/search |
| ---: | --- | ---: | ---: | ---: |
| 1,000 | Hydrate all | 0.0173 / 0.0216 / 0.0216 | 0.0168 / 0.0205 / 0.0213 | 1,000 |
| 1,000 | SQL paged | 0.0098 / 0.0106 / 0.0114 | 0.0095 / 0.0100 / 0.0105 | 1 |
| 5,000 | Hydrate all | 0.0770 / 0.0964 / 0.0974 | 0.0745 / 0.0947 / 0.0951 | 5,000 |
| 5,000 | SQL paged | 0.0251 / 0.0355 / 0.0367 | 0.0248 / 0.0352 / 0.0361 | 5 |
| 20,000 | Hydrate all | 0.2774 / 0.3177 / 0.3420 | 0.2744 / 0.3122 / 0.3389 | 20,000 |
| 20,000 | SQL paged | 0.1073 / 0.1237 / 0.1263 | 0.1063 / 0.1231 / 0.1233 | 20 |
| 50,000 | Hydrate all | 0.7737 / 0.8669 / 0.8768 | 0.7701 / 0.8617 / 0.8717 | 50,000 |
| 50,000 | SQL paged | 0.2751 / 0.3198 / 0.3834 | 0.2740 / 0.3179 / 0.3805 | 50 |

This is a synthetic Python microbenchmark. It does not execute Burrata's Rust
Store implementation or desktop application and does not measure renderer,
IPC, real libraries/filesystems, application/WebView/helper CPU, or steady-state
memory. The CPU values belong only to the benchmark process. No 30-minute
whole-app memory run has been performed; these results do not qualify overall
application performance.
