# Membership migration preservation check

The exact additive v8 migration SQL from `storage.rs` was exercised on a private SQLite-backup copy of the main v7 database. The live database and credentials were not modified.

The comparison covered all original columns and rows across 21 application tables, comparing a sorted multiset of per-row SHA-256 digests before and after. Every existing row/column matched; all catalog listings received saved Library membership; the resulting schema version was 8 and SQLite quick-check returned `ok`. No row data or digests are published here.

This verifies the additive DDL on that snapshot. It does not exercise the complete Store startup path, background jobs, native UI, or live cache/save promotion. The native regressions cover those Store operations with isolated fixtures. The next live app check must confirm the preservation baseline of 4,948 tracks, 36 playlists and 15 assets, then confirm a new search leaves membership unchanged and an explicit save promotes a cached recording.

The original private rollback snapshot is retained separately with directory mode 0700 and database mode 0600. It must not be committed or bundled.


## Follow-up: complete Store startup path to v9

A later preserved-data check used an isolated copy of the retained schema-v7 snapshot and a second disposable schema-v8 copy. Candidate `Store::open` migrated both to v9. All original-column row multisets across 21 tables matched for 4,948 listings; v7 received the historical default membership, the v8 copy retained its membership values, and every new availability mask stayed pending. SQLite quick-check passed and foreign-key check returned zero errors. The source snapshot had only terminal download jobs (10 cancelled, 15 completed, 19 failed); startup reconciliation left those rows unchanged. Aggregate evidence is archived in the [asset-backed v9 checkpoint](../2026-10-16-v9-asset-backed/README.md).
