# Mirror batching: paired development capture

Both captures observed the preserved main profile for 120 seconds with no playback actions and no overlapping builds/tests. One Burrata GUI instance was used sequentially. Both saw two native sync passes over 36 collections and two healthy frontend watcher checks.

| Metric | Before (`8e2bdfa4…`) | Batched (`43e62627…`) |
| --- | ---: | ---: |
| App CPU-seconds | 7.750 | 4.960 |
| One-core CPU median | 1.739% | 1.738% |
| One-core CPU p95 | 75.894% | 5.317% |
| One-core CPU worst sample | 102.142% | 102.749% |
| Mirror phase durations | 2,972 / 2,791 ms | 1,450 / 1,427 ms |
| Peak summed app RSS | 740,601,856 B | 659,398,656 B |

App CPU-seconds were 36.0% lower in this pair. Mean mirror-phase time was 50.1% lower. This supports the repeated-hydration diagnosis for these passes, not a general playback startup or release-build claim. The burst still reaches roughly one core, but lasts less time.

The implementation reuses fetched browser snapshots, batch-hydrates saved track state, and reuses track vectors for unchanged mirrors. An actual original-collection edit still reloads persisted tracks before rendering; personal copies retain their own reads. Deterministic tests cover duplicate occurrences, retained captured durations/provenance, None overwrites, edited output versus fresh store rendering, and a subsequent sync preserving the edit.

This is one exploratory before/after pair, not a randomized or repeated qualification. The earlier executable had been running longer, and identical frontend panel/search state was not independently established, so RSS and complete-app CPU differences cannot be attributed solely to this change. The native per-phase measurements are narrower evidence. Vite recorded 0.000 / 0.010 CPU-seconds. Summed RSS can double-count shared pages; it is not a leak or memory-saving proof. Polling omits short-lived processes and reports storage IO rather than network bytes. Neither capture is a release build, historical WebView comparison or Rust-cutover qualification.

Raw records: [baseline summary](../2026-10-15-mirror-batching-baseline/summary.json), [batched summary](summary.json), and their `profile` / `phase-observations.json` files. `scripts/performance/summarize_playlist_sync.py` computes the same nearest-rank p95 and sums group CPU-seconds without double-counting thread rows.
