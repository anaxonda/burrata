# Runtime two-entry batch observation

One actual saved-track batch was accepted through the existing app MCP. This records a single warm runtime observation, not a performance cohort. No settings were changed.

## Frozen runtime

- App PID: `1386800`; binary SHA-256: `55a641ae2a9e01aaea587fb6fe7ce812e922bdbe9726bb960d2aff2bfc706cc7`.
- Repository HEAD at capture: `9d5a4194111b023ba9252d85039aeb50e7b6fcfa`; worktree had 241 dirty paths. The app binary’s exact source-tree state is not independently bound to that HEAD.
- Matching mode: Balanced; webview playback engine.

## Accepted entries and progress

- One UUID operation `741e24c5-a05f-4579-8f0c-0ba0b13d264a` was accepted with two queue entry IDs: `c5d18b7d-2535-4e92-ad4f-4d6753e92235` (Bacc) and `3111376a-71c2-49ca-a477-ad4ec4e580d8` (Vibe With Me).
- Bacc reached 5.24 seconds of position delta over 5.27 seconds after playing began; the observed position was 5.73 seconds.
- The queue advanced to Vibe With Me at index 1. It reached 5.00 seconds of position delta over 5.15 seconds and was still playing at position 36.49 seconds.
- At the final read, the playback operation registry reported `cancelled` while `playback_state` showed the second accepted entry `playing` at index 1. The registry status is recorded separately; its lifecycle meaning is not inferred. The queue advanced naturally, with no second playback command.

## Logical trace phases

App-log phase counts during the bounded run: `resolution_start` 2, `mapping_resolve_start` 2, `mapped_direct_youtube_start` 2, `mapped_direct_youtube_end` 2, `extraction_start` 0, and `extract_and_validate_start` 0. The full allowlisted phase counts are in `runtime-batch-observation.json`. There were 52 trace events; 51 lacked a native operation ID, so the phase totals are associated with the test window but cannot be joined individually to each queue entry. These are logical phase counts, not HTTP request totals.

## Limits

This demonstrates one two-entry batch traversing two cached source mappings and reaching five seconds of progress on each entry. It does not estimate population performance or test cold resolution. Five seconds of queue-position advance does not establish audibility or acoustic identity. Raw MCP responses remain private, mode 0600, in `.cache/matching-batch-runtime-20261022/`.
