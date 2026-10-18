# BPM index smoke benchmark

This is an exploratory correctness/performance smoke, not a qualification run. It used the isolated candidate test binary and one 1,000-row Store fixture with one distinct zero-byte local asset per listing. The fixture measures metadata/asset hydration and file checks; it does not decode audio.

The benchmark ran three warmups and three measured observations per mode, alternating indexed and no-index order. The no-index control dropped only the two candidate revision-named BPM indexes. It exercised pending behavior and ready BPM ordering on the first and second pages in both directions, and asserted page parity between indexed and no-index results.

The ignored test passed (1 passed, 732 filtered out). Total process time was 5.84 seconds (`user` 5.17s, `sys` 0.24s). The run reported:

- Indexed first-page medians: 88.36 ms ascending, 82.77 ms descending.
- No-index first-page medians: 87.66 ms ascending, 101.82 ms descending.
- Indexed later-page medians: 79.96 ms ascending, 84.25 ms descending.
- No-index later-page medians: 87.94 ms ascending, 92.12 ms descending.
- Initial backfill: 593.83 ms across two batches (largest batch 318.46 ms); populated index build: 23.72 ms.
- Pending first/later page medians: 0.073 ms / 0.066 ms.

At 1,000 rows these exploratory measurements are too small and noisy to establish an index benefit. They only verify the harness and its response/order assertions. The candidate binary was built from isolated source hash `cc32a68cc64910b33996e293e13c559469de70507c72fa9bb0ec0fe935e9b02c` and has SHA-256 `4bb377110322474c87d804a14b1d8d9a0b83cdf345fbfd60bf4b0ea1dc431b3b`.
