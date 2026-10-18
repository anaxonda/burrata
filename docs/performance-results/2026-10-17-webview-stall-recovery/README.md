# WebView stall recovery and playback position

The default WebView path could remain buffering indefinitely when a finite remote source stopped progressing without emitting a media error. Recovery also lost its consumed playback position after successful startup cleared the pending resume value.

The current source adds an owner-bound 20-second media-clock watchdog after completed playback attachment, with cancellation for pause, seeking, source/queue occurrence replacement and unmount. Real clock progress postpones recovery. Repeated waiting/stalled events do not extend the deadline. Finite full remote sources are eligible; local, excerpt, live/radio and unknown media durations are excluded. Recovery captures the actual media position before detaching the source and verifies the replacement honors that position both before and after play. An invalid or unsupported resume fails visibly instead of silently restarting at zero. Existing bounded retries and source pinning remain in effect.

Initial successful attachments now retain recovery ownership so that late failures can use the existing stable-playback recovery policy. Startup and steady playback remain separate budgets.

## Verification in progress

The first complete frontend run passed 956 tests and failed one fixture whose media clock jumped 36 seconds without advancing wall time. The fixture was corrected to represent elapsed playback and a timeupdate before the freeze; the targeted regression now passes and still asserts that a replacement resetting to zero is rejected. The full rerun passes all 957 tests across 79 files in 208.06 seconds. Source hashes confirm only the intentional fixture update changed during verification. Svelte checks report zero errors/warnings and the production frontend build passes (with the existing chunk-size advisory).

Earlier test launches failed before test collection because Vite temporary module writes exceeded the `/tmp` user quota. Moving both TMPDIR and Vite cache to workspace-owned disposable directories resolved that environmental failure. Logs and the bounded owned-cache cleanup ledger are retained under `.cache/qualification-webview-stall-recovery/`.

The sole normal app now runs native26cd66cc0d95 / App0840032816bf. The complete 30-entry queue, occurrence IDs, index, volume, repeat and shuffle are preserved; the previously playing track resumed near its persisted position and subsequent snapshots confirm advancing playback. Restoration was verified after the frontend-mounted handshake. A separate manual-seek completion watchdog is under review because a missing seeked/error event can leave a seek waiting indefinitely; this report does not claim that path is fixed yet. Nor does this establish the cause of the original brief native underflow.

A read-only desktop playbar geometry check at 954×1036 reports no overlapping-control findings (footer112.39px). This is a bounded geometry check, not a complete visual/accessibility audit. Result: `.cache/qualification-webview-stall-recovery/rollout/playbar-layout.json`.

## Bounded current-build replay

The combined 0.1.1 debug executable `ab8904c98d9b` resumes the previously stalled Los Bunkers recording from 277.684804476 seconds on the same canonical Monochrome source. The WebView reports playing at 298.934441476 seconds, then is paused at 305.371934476 seconds. A paused seek restores the original 277.684804476-second position. All 30 queue occurrences, selected index, repeat/shuffle and volume remain intact. This proves a successful bounded replay after the captured stall, not an induced live demonstration of every recovery branch or acoustic output. Deterministic App fixtures cover fresh-source refresh, bounded fallback, failure causes and stale owners. Full frontend regression passes 975 tests across 81 files; combined native regression passes 1,091 with 21 ignored. Evidence is retained under `.cache/release-final-playback/`.
