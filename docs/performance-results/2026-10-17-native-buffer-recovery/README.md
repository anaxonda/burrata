# Current native buffer-recovery investigation

The current Linux debug executable is `228991e0e6c6ac7ee8615978fad979afd86e08fba280f4e051e2770c372c54c3`, with App `9e2c5fee4172dff4fbd07655f5093638eff676974c556a29c2803d0b66b18500`. The build and 1,069 native tests (21 ignored) pass; all 545 pinned build inputs remain unchanged. The complete frontend suite passes 937 tests across 78 files.

## Live native observation

A fresh credential-free, marker-bound profile contains only the pinned Fruit Bats recording `monochrome:239910648984977408`, 181.4 seconds. Monochrome is preferred, fallback and experimental startup reserve are disabled. The app runs alone on an owned null sink. One UUID-bound queued start is issued, with no setup playback, seek or retry. Source and occurrence ownership are checked throughout.

The operation reaches position30.232381 seconds across53 retained observations. No post-start buffering trigger is observed in the30-second progress window. This is another no-reproduction control, not proof that the earlier interrupted operation recovered and not a defect fix. The runner would observe the same generation/seek for5.5 seconds after a trigger; that branch is not exercised by this run. It finishes in36.406 seconds. Owned pause, original volume0.8 restoration and UUID cancellation all pass. All7 pinned helper inputs remain unchanged.

The source-binding disclaimer inside the generic runner is retained. Separately, root verified the sole PID, immutable executable hash, marker/profile/endpoint ownership and source-pinned build receipt. These establish the candidate binding without rewriting the runner result.

## Lifecycle and user state

The first isolated stop attempt incorrectly used the normal MCP discovery endpoint and failed before sending a stop; its failure is retained. The explicitly scoped stop then succeeds. The isolated GUI is stopped and its exact owned null-sink module is removed. The normal app is relaunched through Vite-module and authenticated frontend-mounted checks on the current candidate.

The full normal30-entry queue, occurrence IDs/order, currentIndex1, volume0.8, repeat off and shuffle off compare equal before/after. The pre-stop playing position118.751172s persists as119.501382s (normal progression between snapshot and shutdown). The normal app initially restores paused, then resumes only after that full queue/settings comparison. A subsequent snapshot confirms playing at139.43914s. No provider write or credential mutation was performed.

Private raw results, source/helper pins, stop/relaunch ledgers and full queue snapshots are retained under `.cache/qualification-queued-playback/fruit-native-current228/`.

## Evidence distinctions

The retained underflow remains a real temporary starvation observation; a permanent stall and cause are not established. The downloader five-second setting is a no-data timeout before reconnect, not a fixed post-error sleep. The original1.426-second download-progress age is below that threshold, so it cannot justify tuning reconnect timing.

The Poso YouTube media duration250.079s differs from the245.48s catalog reference by4.599s. It fails the strict qualification±1s gate while remaining inside the product resolver ordinary±5s tolerance (verified integer extractor values have a separate bound). The retained report lacks sufficient source/audio identity evidence to prove it was a different recording. No matcher-tolerance change follows from that evidence.

The earlier95%-seek interruption is not a near-end harness error:9.07s remained at seek dispatch and8.70s at the final observed position, enough for the5-second check. It is a real transient buffering observation. The experimental reserve is startup-only and does not engage at that seek, so the one enabled-run observation does not establish reserve causality.

## Controlled data-gap recovery

The existing ignored paced software-mixer fixture was run alone under a45-second outer timeout against the same pinned native source: `native_output::tests::paced_same_wav_input_gap_preserves_real_consumption_and_exposes_baseline_starvation`. It passes in11.92 seconds. Both arms use identical immutable eight-second PCM WAV bytes. The delayed arm receives only65,536 bytes, exhausts its PCM while the body is held, then releases remaining bytes at two seconds on the paced consumer clock. It resumes without restart and consumes441,448 actual samples (five seconds) by6,690ms. Immediate control consumes441,392 samples by5,020ms with no synthetic-silence starvation. Cancellation/decoder completion/storage-fault assertions pass.

This demonstrates native decoder/mixer recovery after a controlled short network pause, not app UI recovery or a long reconnect/expiry case. It does not erase the earlier live interruption or establish its cause. Raw output is `controlled-gap.log` in the private evidence directory.
