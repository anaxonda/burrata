# BPM projection-index exploratory measurements

These runs use the isolated no-asset projection candidate. They are exploratory three-sample results, not qualification evidence. Each run used an actual Store with 50,000 remote-only library listings, all sort metadata present, page size 200, three warmups and three measured samples per mode. The comparison alternated index-enabled and matched no-index reads on the same rows and projections; the control dropped only the two candidate BPM indexes. It checked first and later pages in both sort directions and asserted identical row order.

The fixture distributions are intentionally separate. `tied-128.log` assigns BPM 128 to every row. `varied-80-199.log` uses `80 + index % 120`, producing repeated BPM values from 80 through 199. Both included a correctly ordered raw-metadata BPM page while availability was pending. The indexed path ran only after backfill completed and EXPLAIN coverage admitted the revision-qualified index.

| Fixture | Indexed medians ASC first/later | Indexed medians DESC first/later | No-index medians ASC first/later | No-index medians DESC first/later |
| --- | ---: | ---: | ---: | ---: |
| Tied 128 BPM | 341.6 / 308.2 ms | 328.7 / 296.4 ms | 1341.2 / 1364.7 ms | 1343.9 / 1336.9 ms |
| Varied 80–199 BPM | 315.3 / 316.3 ms | 329.2 / 304.6 ms | 911.0 / 885.7 ms | 917.5 / 869.9 ms |

Each run's reported p95 is the maximum of only three measured samples. The 50k backfill took 45.29s for tied BPM and 45.87s for varied BPM, in 100 batches of up to 500 rows; maximum batch times were 569 ms and 535 ms respectively. Runtime background batches use a 250-row limit, so these numbers are not a UI stall measurement. Populated index creation took 1.13s and 1.15s. Index maintenance across repeated reset/backfill cycles was not measured.

The test binary is immutable at `/tmp/burrata-sorting-target/debug/verified-tests-v9-noasset-index-tied-varied-7dd3d0ba/burrata_lib`, SHA-256 `e458ef36c3d4c3ecb44cab6633a9194f428e83190e9527bbb6997c31d2c09c5d`. Its isolated source SHA-256 is `7dd3d0ba3d1a06cc3069cc8f0bb5b875a1971507ffd260e9ad6281b1155564b9`.

The commands used `TMPDIR` and `SQLITE_TMPDIR` set to `/home/loki/projects/burrata/.cache/native-test-temp`, `BURRATA_AVAILABILITY_BENCH_COUNTS=50000`, `BURRATA_AVAILABILITY_BENCH_SAMPLES=3`, `BURRATA_AVAILABILITY_BENCH_ASSETS=0`, `BURRATA_AVAILABILITY_BENCH_SPARSE=0`, and `BURRATA_AVAILABILITY_BENCH_INDEX_MAINTENANCE=0`. `BURRATA_AVAILABILITY_BENCH_TIED=1` selected the constant-128 fixture; `=0` selected varied BPM. The executed test was `storage::tests::benchmark_bpm_projection_index_pages_and_maintenance --exact --ignored --nocapture`.

No 20-sample qualification run or asset-backed 50k run has been performed yet.
