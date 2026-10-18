# 50k actual-Store text-index experiment — debug build

The isolated revised harness passed in 806.52s. It checked all 50,000 ordered occurrence IDs for title/artist in both directions, checked the same orders after 200 metadata updates and projection backfill, then reopened the database and verified all four indexes remained. The 160 measured pairs use actual 200-row Store pages, alternating baseline/indexed order, after three warmups per arm. P95 is nearest rank.

| Field/order | Offset | Baseline median/p95/worst (ms) | Indexed median/p95/worst (ms) |
|---|---:|---:|---:|
| title asc | 0 | 1239.81/1328.57/1353.30 | 134.67/146.06/147.74 |
| title asc | 25000 | 6611.56/7384.59/7395.21 | 242.87/282.05/386.43 |
| title desc | 0 | 1173.50/1212.06/1266.76 | 132.24/143.87/156.44 |
| title desc | 25000 | 6211.03/6508.11/6650.83 | 245.79/261.80/262.76 |
| artist asc | 0 | 1075.71/1121.40/1121.88 | 137.85/145.63/152.86 |
| artist asc | 25000 | 4583.91/4738.55/4752.39 | 241.34/263.02/288.52 |
| artist desc | 0 | 978.94/1075.68/1102.99 | 130.78/144.64/152.06 |
| artist desc | 25000 | 4626.99/4722.57/4797.64 | 256.03/282.47/288.46 |

Initial index creation/open took 18.299s versus 7.762ms baseline. Database size grew by 3,588,096 bytes (126,918,656→130,506,752). The single 200-edit/backfill batch took 885.32ms baseline and 1176.18ms indexed; this is one batch, not write p95 qualification. Existing-contract indexed reopen took 8.128ms without rebuilding.

The experiment remains isolated and default-off; production Store code and the normal profile did not acquire these indexes. This establishes finite read/order behavior, not adoption. Matched release-build, mutation-class, whole-app/IPC/CPU/memory and platform qualification remain open. The partial resource sample covers the combined benchmark child, not separate arms or a full run.

The earlier intentionally stopped run is retained separately in [the partial report](../2026-10-17-text-index-partial/README.md). The one-pass parity collector uses the same Store query builder/comparator and compares every occurrence; the timed page API remains unchanged.

Local evidence: `.cache/text-index-one-pass-actual-store-50k.log`, `.cache/text-index-one-pass-candidate.json`, `.cache/text-index-one-pass-focused.log`, `.cache/text-index-one-pass-resource-sample.json`; exact machine summary: [result.json](result.json).
