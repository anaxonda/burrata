# BPM index stage profile

This was one profile diagnostic run against the same immutable binary and 50,000-row all-BPM-tied fixture as the exploratory benchmark. The environment requested one measured sample, but the harness clamps its minimum to three; the invocation therefore performed three measured samples per mode plus three warmups. The ignored test passed (1 passed, 732 filtered out); total process time was 123.39 seconds (`user` 112.23s, `sys` 9.00s). This is diagnostic data, not a qualification run.

The test-only stage hook reported no local assets or page-effective metadata. Hydration/materialization work was near zero; page-row decoding/hydration took about 47–50 ms. The page SQL stage dominated both before and after backfill:

| State and page | Count/availability or count/coverage median | Page SQL median | Page hydration median | Total median |
| --- | ---: | ---: | ---: | ---: |
| Before backfill, first | 140.3 ms | 1128.7 ms | 49.3 ms | 1409.3 ms |
| Before backfill, later | 139.0 ms | 1111.9 ms | 47.0 ms | 1416.6 ms |
| Ready, first | 157.4 ms | 1106.0 ms | 47.5 ms | 1408.9 ms |
| Ready, later | 132.6 ms | 1140.9 ms | 47.2 ms | 1419.2 ms |

These values point to page SQL as the dominant cost. This diagnostic did **not** use the indexed page-query branch: the no-asset backfill left `sort_metadata_projection` NULL, so the filtered projection-coverage gate rejected the partial BPM indexes. The page-SQL profile therefore reflects sorting the full relation, not index performance. A separate test asserts the generated BPM index query plan for offsets 0 and 1, but the candidate source extension for that assertion was not compiled or run in this diagnostic. The test’s old log field `pending=true` before backfill represented expected availability-pending state, not actual `sortPending`; for no-asset rows the sort was still performed against raw metadata. The isolated source now reports these states separately.

Profile source was isolated candidate hash `cc32a68cc64910b33996e293e13c559469de70507c72fa9bb0ec0fe935e9b02c`; binary SHA-256 `4bb377110322474c87d804a14b1d8d9a0b83cdf345fbfd60bf4b0ea1dc431b3b`.
