# Experimental text-index read pilot — incomplete

The isolated 50,000-track actual-Store run was intentionally stopped after 904.83 seconds (exit 101). Its repeated paginated full-order checks could not finish the four field/direction cases plus update/reopen checks within the deadline. This is not a completed performance qualification. Both owned processes exited. No production Store code or normal-profile flags changed.

Twenty alternating paired 200-row reads completed for title ascending at each offset, following three warmups:

| Offset | Baseline median / p95 / worst (ms) | Indexed median / p95 / worst (ms) |
|---|---|---|
| 0 | 1177.67 / 1283.25 / 1303.03 | 131.91 / 145.95 / 163.34 |
| 25,000 | 6502.28 / 6894.65 / 6950.16 | 239.02 / 256.12 / 258.86 |

P95 uses nearest rank. Full 50,000-ID parity passed for title ascending before these reads. Other directions, artist sorting, post-update parity, update overhead and reopen costs remain unqualified. Initial indexed open/build took 17.90 seconds versus 7.02 milliseconds baseline; database size increased by 3,588,096 bytes. The prototype remains disabled. These are debug-build Store measurements, not release, whole-app or playback timings.

The revised harness retains all-ID parity through the same Store query builder using one complete prefix read instead of repeated 1,000-row hydrated pages. Timed 200-row API reads remain unchanged. This harness revision still needs compilation and a complete run.

Local raw evidence: `.cache/text-index-actual-store-50k.log`, `.cache/text-index-actual-store-partial-summary.json`, `.cache/text-index-actual-store-partial-stop.json`; source-only harness proposal: `.tmp/full-order-qualification-proposal/`.
