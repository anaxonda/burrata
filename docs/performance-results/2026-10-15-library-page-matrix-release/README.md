# Release library page matrix

This run exercised the real `list_tracks_page` MCP command against isolated synthetic libraries of 1k, 5k, 20k, and 50k tracks. Each of five scenarios used a page size of 200, three warmups, and 20 measured reads. All measured reads passed identity/order parity and continuation checks; failures were zero throughout. The tested release binary SHA-256 was `67025d222c61a8dbb1189f7408028155104f0e1def189ee6cacc723298424f4c`.

Latency values are milliseconds (median / p95 / worst). CPU is aggregate Burrata executable user+system CPU seconds across the 20 measured reads. Each scenario's full report is linked in the [matrix index](matrix-index.json).

| Tracks | Scenario | Latency ms (median / p95 / worst) | App CPU s / 20 reads | Failures |
|---:|---|---:|---:|---:|
| 1k | Title ascending | 40.4 / 48.4 / 66.8 | 0.64 | 0 |
| 1k | Artist ascending | 37.6 / 47.4 / 50.4 | 0.65 | 0 |
| 1k | Duration ascending | 43.0 / 49.1 / 58.8 | 0.80 | 0 |
| 1k | Duration descending | 47.8 / 54.8 / 62.5 | 0.87 | 0 |
| 1k | Unicode query, source order | 28.9 / 30.0 / 31.2 | 0.57 | 0 |
| 5k | Title ascending | 76.3 / 84.7 / 117.7 | 1.37 | 0 |
| 5k | Artist ascending | 81.2 / 94.3 / 95.7 | 1.57 | 0 |
| 5k | Duration ascending | 121.9 / 128.2 / 128.9 | 2.38 | 0 |
| 5k | Duration descending | 130.8 / 138.9 / 140.2 | 2.48 | 0 |
| 5k | Unicode query, source order | 145.1 / 156.4 / 187.3 | 2.90 | 0 |
| 20k | Title ascending | 219.0 / 240.9 / 255.9 | 4.44 | 0 |
| 20k | Artist ascending | 205.3 / 222.9 / 244.4 | 3.94 | 0 |
| 20k | Duration ascending | 383.5 / 420.0 / 420.8 | 7.57 | 0 |
| 20k | Duration descending | 441.6 / 478.8 / 502.6 | 8.70 | 0 |
| 20k | Unicode query, source order | 581.5 / 621.3 / 646.9 | 11.60 | 0 |
| 50k | Title ascending | 485.9 / 527.2 / 552.7 | 9.86 | 0 |
| 50k | Artist ascending | 471.9 / 510.8 / 514.9 | 9.25 | 0 |
| 50k | Duration ascending | 915.5 / 967.4 / 983.0 | 18.18 | 0 |
| 50k | Duration descending | 1087.0 / 1150.6 / 1155.6 | 21.64 | 0 |
| 50k | Unicode query, source order | 1420.6 / 1467.6 / 1500.5 | 28.12 | 0 |

The Unicode-query scenario has 1, 5, 20, and 50 matching rows respectively at these fixture sizes; the other scenarios report the complete fixture total. This is a synthetic data workload, not a user library. CPU values sample only the Burrata executable around each MCP call; transport framing is excluded from serialized-result byte measurements. The matrix measures backend paging and MCP reads, not UI rendering or shared system RSS.

The separate [debug matrix results](../2026-10-15-library-page-matrix-debug/matrix-index.json) use a different binary/revision. These results do not establish a release-versus-debug speedup.
