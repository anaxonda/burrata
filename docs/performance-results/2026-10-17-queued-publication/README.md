# Playback acceptance and published queue consistency

The cached album run exposed a WebView acceptance receipt arriving before the periodically published playback snapshot contained its queue. Acceptance now carries a full exact playback projection to the backend. The backend validates the receipt, queue occurrence IDs and current occurrence, publishes through the existing ordered protocol and commits acceptance under the operation-registry lock. Invalid, cancelled or superseded requests cannot publish unrelated state. Publication acknowledgements preserve failure-history acknowledgement semantics, with bounded retry for stale packets.

The frontend waits for its current occurrence to be anchored before producing that acceptance snapshot; cancellation and replacement reject the wait. Five mounted regressions cover immediate publication, stale acknowledgement retry, bounded failure, cancellation during acknowledgement and cancellation before occurrence anchoring. The complete integrated frontend suite passes: 909 tests across 76 files, with one worker.

Independent review found a separate native race between copying an acceptance snapshot and committing its receipt. Native receipt commit now runs while the existing playback command mutex is still held, preserving command-to-registry lock order. Competing replacement commands cannot enter that interval. Two deterministic tests cover serialization and rejected commit; they use the actual commit boundary without requiring a device or Tauri application.

The first full native run compiles, then ends with 1,041 passed, one failed and 21 ignored. The new race fixture moves its completion receiver into the commit callback; after that callback returns, the competitor sends to the dropped receiver and its `unwrap` panics. This is a test-fixture failure, and the original failed log is retained. The test-only correction now proves the competitor encountered the held mutex and checks a real cancelled registry row retains no receipt. Implementation bytes are unchanged. The corrected full native suite passes: 1,042 passed, zero failed and 21 ignored, in 17.55 seconds. All 541 pinned candidate inputs remain unchanged before starting the executable build. Live acceptance, cancellation and the cached album comparison remain open. The original failed album run and corrected cleanup follow-up are preserved; no live playback success is claimed here.

Integration backups, hashes and logs are retained under `.cache/qualification-queued-publication/` and `.cache/qualification-native-acceptance/`.

Current frontend candidate checks also pass Svelte (zero errors/warnings), the production build, eight contract checks and all 12 MCP bridge tests with host loopback access. The sandboxed MCP server test failed and its log remains separate. These checks do not qualify live playback.

The executable build passes. Its immutable candidate is native `97597ce4c7fd51131e25f6ad67398a670f871a0337cbf18e80f130256856f1da`, frontend App `a2f1055c10d819cced3e1174c3a3a26f512d5a3de375cdfc8cf11551e5421ea0`; all 541 pinned inputs were verified unchanged after build. Live qualification remains pending.

### Current live verified-cache album run

The sole isolated candidate mounted through the guarded launcher. Before playback its queue was empty, volume 0.8, repeat off and shuffle disabled. Public app-owned calls warmed the exact release and three credited artist profiles; a read-only attestation verified 19 catalog identities/titles/artists/durations and four fresh raw-cache identities. This is a catalog-warm test, not a network-cold measurement.

The queued API then completed all 19 exact occurrences in 80.479 seconds without the earlier queue/status mismatch. Every occurrence retained an owned unavailable-source outcome; the operation ended exhausted with matching immutable receipt, verified cleanup and restored volume. No eligible media was observed, so this verifies the bounded acceptance/finite-traversal branch only, not playable sources, acoustic correctness or startup performance. Private report: `.cache/qualification-queued-publication/full19-verified-cache-run-private.json`.

The current actual-app buffering-state capture at 1914×515 shows a 98.5px footer, loaded artwork and no layout findings; it does not qualify half-width or other player states. The original failed cached run remains retained.
