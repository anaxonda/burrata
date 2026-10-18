# Asset-backed BPM index exploratory profile

Exploratory three-sample run only; not qualification evidence. This actual-Store fixture had 50,000 remote listings, one distinct zero-byte local file and one local asset payload per listing, varied BPM values 80–199, and sparse remote metadata. It measured asset descriptor/hydration paths only and did not decode audio. The matched control dropped only the two candidate revision-named BPM indexes on the same rows and projections.

| Page | Indexed median | No-index median |
| --- | ---: | ---: |
| Ascending, first | 547.4 ms | 1111.1 ms |
| Ascending, later | 517.1 ms | 1070.0 ms |
| Descending, first | 554.6 ms | 1088.3 ms |
| Descending, later | 508.7 ms | 1013.2 ms |

Each median uses only three measured samples after three warmups; p95 is the maximum sample. Indexed asset-backed pages are above the exploratory 500 ms target. Before projection backfill, first and later BPM pages correctly returned `sortPending=true` with no rows (median ~0.105/~0.101 ms). Backfill took 87.96s over 100 batches of up to 500 rows (largest batch 982ms); the populated index build took 1.07s. Runtime background batches use 250 rows, so the benchmark batch time is not a UI stall measurement.

The test-only stage profile emitted 48 mixed first/later and indexed/no-index records. Across those records, median asset discovery was 104ms, count/availability 103ms, page SQL 257ms, page hydration 104ms, and total request time 813ms. The page SQL metric combines index-backed and full-sort requests, so it is diagnostic rather than an indexed-page estimate.

The immutable binary SHA-256 is `e458ef36c3d4c3ecb44cab6633a9194f428e83190e9527bbb6997c31d2c09c5d`; source SHA-256 is `7dd3d0ba3d1a06cc3069cc8f0bb5b875a1971507ffd260e9ad6281b1155564b9`. Command environment: workspace `TMPDIR`/`SQLITE_TMPDIR`, `BURRATA_AVAILABILITY_BENCH_COUNTS=50000`, `..._SAMPLES=3`, `..._TIED=0`, `..._ASSETS=1`, `..._SPARSE=0`, `..._INDEX_MAINTENANCE=0`, and `BURRATA_AVAILABILITY_PROFILE=1`; test `storage::tests::benchmark_bpm_projection_index_pages_and_maintenance --exact --ignored --nocapture`. Raw output is in [asset-profile.log](asset-profile.log).

No 20-sample qualification run has been performed.
