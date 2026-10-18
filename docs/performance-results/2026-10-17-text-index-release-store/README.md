# 50k actual-Store text-index experiment — release build

The frozen release candidate passed the isolated actual-Store qualification. Across 160 paired 200-row page reads (20 per case), indexed reads were faster in all eight title/artist, ascending/descending, offset-0/offset-25,000 cases. The harness also checked all 50,000 ordered occurrence IDs in four orders before and after 200 metadata edits and projection backfill, then reopened the database and verified all four indexes persisted.

| Field/order | Offset | Baseline median/p95/worst (ms) | Indexed median/p95/worst (ms) |
|---|---:|---:|---:|
| artist asc | 0 | 328.89/347.74/359.40 | 55.37/62.29/63.96 |
| artist asc | 25000 | 755.68/825.50/839.91 | 120.65/149.14/158.62 |
| artist desc | 0 | 334.96/345.59/367.31 | 55.90/60.54/73.27 |
| artist desc | 25000 | 765.00/816.68/856.50 | 129.87/147.90/148.61 |
| title asc | 0 | 342.66/378.77/383.62 | 53.66/62.60/62.67 |
| title asc | 25000 | 930.06/993.45/1048.34 | 116.69/132.13/132.26 |
| title desc | 0 | 341.04/386.99/412.60 | 54.70/57.87/67.65 |
| title desc | 25000 | 895.84/935.47/943.25 | 113.11/135.25/136.62 |

Creating the four indexes during initial open took 1.5145 s, versus 2.0 ms for the baseline open. The indexed database was 130,506,752 bytes versus 126,918,656 bytes for baseline, an increase of 3,588,096 bytes (about 2.8%). The 200-edit/backfill batch took 264.81 ms indexed and 174.15 ms baseline; this is one measured batch, not a write-latency distribution. Reopening an already indexed database took 1.96 ms and retained all four indexes.

The complete command took 171.61 s wall time and peaked at 154,676 KiB RSS (about 151 MiB); it used 131.46 s user and 39.27 s system time. The 87 resource samples recorded a maximum scratch footprint of 266,550,424 bytes (about 254 MiB), with the indexed database the largest sampled file at 130,949,120 bytes. Sampled free space bottomed at about 3.00 GiB. These samples describe this run and environment, not a general resource bound.

The retry used the immutable release binary with SHA-256 `1bd1b429dbbda8413169acdfef844b21486b7fe8078981a983f7180fdd8910d9`; its staged `storage.rs` source pin was `bc770b49082a209443b4143aa1b8c5a98e0bbf67e3a3009f5f51c6ea6524f285`. The strict machine summary is [result.json](result.json). The earlier debug-build report is [the actual-Store debug report](../2026-10-17-text-index-actual-store/README.md).

The debug and release runs both show the indexed arm outperforming the baseline within their respective runs. Their absolute times do not isolate a build-profile effect: the debug run used a workspace `TMPDIR`, while this release retry used a fresh `/tmp` scratch directory for both `TMPDIR` and `SQLITE_TMPDIR`. Treat the matched baseline/indexed comparisons within this release run as the relevant result; do not infer that optimization alone caused the difference between reports.

The first release attempt remains preserved in `.cache/qualification-text-index-release/run.log` and exited after about 31 seconds with a SQLite `disk I/O error` during availability backfill. After cleanup of old YT-DLP assets and dedicated scratch data, the retry passed using fresh temporary storage. This sequence does not establish that quota or any single storage condition caused the first failure. Retry artifacts include `.cache/qualification-text-index-release/retry-run.log`, `retry-time.txt`, and `retry-resource-samples.jsonl`.

This is an isolated Store qualification, not whole-app, audio, platform, or production qualification. The proposal remains default-off; production Store behavior is unchanged, and this result does not promote the indexes for adoption.
