# Helper cleanup on outer cancellation

Source fix integrated; native compilation and regressions remain pending. The running verification app still uses the unchanged immutable executable `97597ce4c7fd`.

An outer playback cancellation/deadline can drop `run_tool_until` before its inner cancellation branch runs. Tokio child kill-on-drop protects the direct helper but does not kill its Unix process group, and dropped reader-task handles otherwise detach. Resource guards now signal the spawned process group synchronously on drop and abort its pipe readers. The leader is not reaped until both pipes close, preserving group identity while a descendant holds the pipes. Normal completion still requires successful exit and closed pipes within the original deadline.

Three new Linux regressions exercise outer cancellation, outer expiry and an exited leader with a descendant retaining the pipes. Their fixtures self-expire even on baseline failure. Independent source review found no Unix-scope blocker; no passing native-test result is claimed yet. Private integration evidence and original source are retained under `.cache/qualification-helper-drop-cleanup/`.

The guarantee covers Unix descendants remaining in the spawned process group. Windows descendant ownership and descendants deliberately leaving the process group remain unqualified. No recording-matching, retry-limit or error-classification change is intended.

Native validation now passes: 1,051 tests, zero failures, 21 ignored in the host run (17.11 seconds). The initial sandbox run is retained: 1,016 passed, 35 failed (34 localhost permission failures and one signal-fixture timeout), 21 ignored. The three new process-group regressions pass in both runs; the actual HTTP alias/deadline/cancellation regressions pass with host socket access. All 541 pinned source inputs remained unchanged through testing. The frontend remains unchanged from the 913-test validated candidate. Executable rebuild and live acceptance remain pending.

The executable build now passes (128 seconds, development/debug), with all source pins unchanged. Immutable native `0a1a4b49455f` has launched alone in a fresh isolated profile and its authenticated frontend handshake passes. Live behavior remains to qualify.

### Controlled live slow-search attempt

The first controlled request was rejected before search because the private fixture omitted required Track fields. Its failed receipt is retained; the fixture was repaired offline using the established full serializer, preserving its identity/title/artist/duration and no requested version. Five abandoned partial preparations were removed after quota failures. Later, 35 hash-identical generated tones in inactive isolated profiles were hardlinked to one canonical file, retaining all paths/content and reclaiming555,663,360 allocated bytes; the active profile was excluded. No normal data or provider credentials were changed.

The corrected request was accepted, and the deterministic helper entered its60-second search stall. The owned operation later failed with the startup-budget-exceeded reason and returned to idle; original volume restoration passed. However, no correlated `resolution_end` event was emitted. The manual observation timed out waiting for that event, so this does not qualify the20-second timing bound. The outer deadline drops the nested resolver before its explicit terminal tracing executes; a terminal-trace guard fix is being staged. Private receipt/trace-reader evidence is retained in `search-deadline-run02-*`; missing terminal evidence is not a pass.
