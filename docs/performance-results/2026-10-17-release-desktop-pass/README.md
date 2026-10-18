# Final bounded desktop pass

The combined debug executable `ab8904c98d9b` launches alone through the verified Vite/module/MCP handshake. Exact 30-entry queue and occurrence IDs, selected index, paused position, volume, repeat and shuffle settings survive relaunch.

Actual Library capture at 1440×900 shows compact controls and no overlapping playback-bar elements. The initial real 954×1036 Search capture exposes a clipped native WebKit sort label; a CSS-only correction applies zero vertical padding and the working source selector's sizing. The post-reload 952×1034 capture shows the complete “Source order” label and no playback-layout findings. Only Burrata's Sway container was temporarily floated/resized; its original tiled mode and Library route were restored.

The user's exact YouTube query returns 13–15 relevant collected results in these bounded reads. Existing exact-ID evidence for the expected Irene video is retained in the [query-filter report](../2026-10-17-youtube-query-filter/README.md); rankings and counts vary. This is not a claim of exhaustive provider search or every desktop route/theme.

Full frontend regression before the final CSS-only and optional identity-hint wiring passes 975 tests across 81 files. The CSS production build and Svelte check pass with zero errors/warnings. The optional identity-hint component's five focused tests pass. Final combined frontend checks pass 978 tests across 81 files. Native `4bfab35be85c` shows the verification action in the actual reachable Details panel at 952×1034. Its source/permission contracts pass and the exact full queue/settings/original paused position remain preserved.

Private screenshots, viewport reports, source pins, launch logs and exact restoration proofs are retained under `.cache/release-final-playback/`; no private runtime artifacts are included in release sources.

## Final local release checks

The complete Cargo command passes all 1,092 library tests (21 ignored), binary test targets and doc tests. A test-only platform correction keeps `/tmp` profile fixtures Linux-specific. A storage-fault fixture had a circular server/decoder read-ahead wait; legal bounded short reads around the real downloader preserve first-sample-before-fault ordering, sample/fault/session assertions and owned-child cleanup, while failing explicitly on an out-of-prefix probe instead of hanging. The corrected full run passes. An independent static review confirms those assertions are preserved. The refreshed production executable is byte-identical to the live-qualified `4bfab35be85c`.

Generated build caches were retired only after SHA/inode/process checks, preserving the running build and a verified rollback. Local `.tmp` browser/test profiles are ignored and absent from staged release sources. No user library data or credentials were removed. Tagged platform artifacts remain a separate gate.
