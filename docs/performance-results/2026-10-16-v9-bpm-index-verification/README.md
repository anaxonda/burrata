# v9 BPM-index candidate: bundled SQLite and native-test diagnostics

This is functional verification for an isolated v9 candidate, not a performance qualification. The candidate is not promoted. No timing benchmark was run for the BPM indexes or their write/backfill cost.

## Candidate identity

- Test executable: `.cache/verification-target/debug/verified-tests-v9-bpm-index-final-df7f918be70a/burrata_lib`
- Test executable SHA-256: `df7f918be70af613fb1420f02cbc98a12649ad6271b1f4bf77ade7f94be0cc04`
- Isolated source root: `/tmp/burrata-availability-native-prototype/src-tauri`
- `storage.rs` SHA-256: `93f8ebeee1292b0ee53f3dbeff6cced25496bb0dbe8b7fa877153f8dbc47cbea`
- `track_sort_sql.rs` SHA-256: `4a25d015700a1f14b5ef1c0e9b789023ffd897ab98220b9fb3b0a160bad09c19`
- `playback.rs` SHA-256: `9c2096b8f03525b643c1ee3a3b57dd56cd204c2ba42236416a762e20d8c5c3e5`
- `remote_playlists.rs` SHA-256: `ca814d4c654fefbbfa75675ec6dfe4bed242832b602d08d675d0c7bfc9bba7d2`

The candidate adds revision-qualified partial BPM sort indexes and an exact filtered-projection coverage gate. The index path is enabled for canonical Library BPM pages only when every row in the filtered relation has a current projection and there are no dynamic page-effective rows. First-page responses still include availability; later pages compute coverage without emitting availability. Missing/stale projections and dynamic rows keep the existing exact-sort fallback. The bundled SQLite regression checks ascending and descending null-last order, stable ties, page-two parity, the filtered local subset, unfiltered fallback, and `EXPLAIN QUERY PLAN` use of each index.

## Results

| Check | Result |
| --- | --- |
| Focused bundled Store test `storage::tests::bpm_projection_index_keeps_filtered_order_nulls_last_and_stable_ties` | 1 passed, 0 failed |
| Storage suite from the candidate executable | 72 passed, 0 failed, 2 ignored |
| Full native suite with default `/tmp` (parallel) | 716 passed, 1 failed, 15 ignored; cooldown test failed on a SQLite disk I/O error during `save_track` |
| Exact cooldown-test rerun from the same executable with default `/tmp` | 1 passed |
| Full native suite with default `/tmp` and `--test-threads=1` | 716 passed, 1 failed, 15 ignored; the 20k-row import-source index fixture failed with `SQLITE_IOERR_WRITE` (extended code 778) |
| Exact 20k-row import-source fixture rerun with default `/tmp` | Failed again with `SQLITE_IOERR_WRITE` (extended code 778) |
| Exact 20k-row import-source fixture with workspace `TMPDIR` | 1 passed |
| Full native suite with workspace `TMPDIR` and `--test-threads=1` | 717 passed, 0 failed, 15 ignored |

The passing alternate-temp command was:

```sh
TMPDIR=/home/loki/projects/burrata/.cache/native-test-temp \
  .cache/verification-target/debug/verified-tests-v9-bpm-index-final-df7f918be70a/burrata_lib \
  --test-threads=1
```

The failing database writes were under Rust's default temporary directory, `/tmp`. At the time of inspection, `/tmp` was a tmpfs mounted with `usrquota`, with about 1.6 GiB of global free space; the workspace ext4 filesystem had about 335 MiB free. The same 20k-row test failed under `/tmp` and passed under the workspace temp directory. The frontend test runner also reported `EDQUOT` under its default temp setup. This evidence is consistent with per-user temporary-filesystem quota pressure, but quota counters were unavailable in the environment, so that cause is not directly measured.

The full native suite is green from the immutable candidate executable when test temp files use the workspace filesystem. The default-`/tmp` failures are retained here rather than reclassified as product failures or hidden by the isolated pass. These runs are functional checks only; they do not establish BPM-index read latency, index maintenance cost, migration cost, or 50k-row performance.
