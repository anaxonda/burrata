# BPM index 50k exploratory benchmark

This is an exploratory three-sample measurement, not a qualification run. It used an actual Store fixture with 50,000 remote-only library listings and all sort metadata known. No assets or audio decoding were involved. The fixture used BPM 128 for every row, so it stresses a large tie group rather than a varied BPM distribution. Each mode had three warmups and three measured observations. Indexed and no-index observations alternated order. The matched no-index control dropped only the two candidate revision-named BPM indexes. The test checked first and later pages in both directions, stable ordering, and matching IDs/order between indexed and no-index modes. Its pre-backfill `Pending` label in this run meant global sort availability was pending; the no-asset BPM page itself was not `sortPending` and raw-metadata-sorted rows were returned. The harness now reports those states separately.

The ignored test passed (1 passed, 732 filtered out). Total process time was 123.78 seconds (`user` 112.86s, `sys` 8.70s). Since each summary has only three measurements, the reported p95 is the maximum sample, and these results are not qualification evidence.

| Page | Indexed median / p95 max | No-index median / p95 max |
| --- | ---: | ---: |
| Ascending, first page | 1388.96 / 1464.95 ms | 1485.61 / 1496.32 ms |
| Ascending, later page | 1396.33 / 1502.75 ms | 1416.85 / 1508.17 ms |
| Descending, first page | 1477.49 / 1491.42 ms | 1379.81 / 1499.72 ms |
| Descending, later page | 1453.65 / 1487.28 ms | 1392.77 / 1414.31 ms |

There is no consistent index win in these measurements, and all ready-page medians are well above the 500 ms target. First/later pending-state page medians were 1375.89 / 1394.55 ms. The initial 100-batch metadata backfill took 37.75 s (largest batch 462.78 ms); building the populated BPM indexes took 101.57 ms.

This run did **not** exercise the indexed page-query branch. The old backfill cached an availability mask for no-asset rows but left their `sort_metadata_projection` NULL; coverage therefore rejected the partial BPM indexes and the page SQL sorted the full relation. The indexed/no-index timings above compare the same fallback query, with the two index definitions present or absent. They are not evidence that the index is ineffective when eligible. A later isolated patch stores the exact no-asset raw-plus-manual effective JSON and adds EXPLAIN coverage tests, but that patch has not been benchmarked.

The fixture used exploratory sample count 3. Further changes require profile-guided optimization and correctness checks before any larger qualification run. The candidate binary was built from isolated source hash `cc32a68cc64910b33996e293e13c559469de70507c72fa9bb0ec0fe935e9b02c` and has SHA-256 `4bb377110322474c87d804a14b1d8d9a0b83cdf345fbfd60bf4b0ea1dc431b3b`.
