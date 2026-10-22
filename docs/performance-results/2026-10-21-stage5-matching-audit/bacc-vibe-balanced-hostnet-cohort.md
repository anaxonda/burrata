# Bacc and Vibe Balanced resolver cohort (host network)

The bounded two-case cohort completed with all attempts successful. It used the frozen `qualification-alias-cache-bpm-v3` diagnostic binary, the same public settings for both catalog fixtures, and `exec_command` with `require_escalated` host networking. Before the cohort, a one-pair Vibe smoke with the same binary/settings also succeeded cold and warm (8.804 s cold, 0.581 s warm, same YouTube ID `ExdT24gJngk`). The coordinator runs cases serially; this is not an interleaved Bacc/Vibe-per-repetition schedule.

| Fixture | Cold successes | Cold p50 / p95 (ms) | Warm successes | Warm p50 / p95 (ms) | Cold / warm source identity changes |
|---|---:|---:|---:|---:|---:|
| Bacc Of The Bach | 20/20 | 10065.2 / 11455.4 | 20/20 | 424.4 / 850.8 | 0/20 |
| Vibe With Me | 20/20 | 9304.3 / 11679.5 | 20/20 | 364.5 / 1133.8 | 0/20 |

All 40 attempts per fixture resolved the same YouTube ID within that fixture: Bacc `rpCswezRrLs`; Vibe `ExdT24gJngk`. The two fixtures resolved to different IDs, as expected. This measures the production resolver and diagnostic decode path, not app queue behavior, playback UI, acoustic identity, or the current/new download route. It should not be attributed to the `ed056432…` production app artifact.

Trace correlation succeeded for all measured attempts. In each cold scenario, Bacc emitted 20 logical `extraction_start` and `extraction_end` events, 40 preview-audio lookup starts/ends, and one verified-source transport-recovery start/end pair. Vibe emitted the same extraction and preview lookup counts and two such recovery pairs. Each warm scenario emitted 20 mapped direct-YouTube starts/ends and 20 mapping-resolution starts/ends, with no logical extraction starts. These are allowlisted logical phase counts, not physical extraction counts or HTTP request totals. The separately reported observed resolver extraction-budget counter snapshots have different scope: cold sums were 2 (Bacc) and 4 (Vibe), while warm sums were 20 for both; zero/low snapshots do not override the logical phase event counts.

Settings policy was preferred Monochrome with fallback enabled to YouTube and SoundCloud, startup timeout 20 seconds. There were no measured failures; each cold and warm p95 uses 20 measured timing observations. Binary SHA-256: `7a95ec034871cfbd205ed022fd634e7ae8c97d71e43f0a985fbf49eb25726f6a`; diagnostic example source SHA-256: `02abc81d239dc70f713a61d06f170964cad6bf06bc05ee0432bbc680b00e85fe`; two-case input manifest SHA-256: `7202ffdf4fc69a9e927dc1b7c1cad611b7b8a0cd68608671849d4531ee58176a`; repository revision: `9d5a4194111b023ba9252d85039aeb50e7b6fcfa`.

The [sanitized aggregate](bacc-vibe-balanced-hostnet-cohort.json) includes per-case hashes, source identities, scenario summaries, and phase counts. Raw reports and logs remain in `.cache/matching-stage5-two-track-balanced-hostnet-cohort-20261021/` with owner-only permissions.
