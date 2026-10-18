# Resolution terminal tracing on outer cancellation and expiry

Source changes pass all 1,060 native tests and the development/debug executable build. Five fresh isolated Linux WebView helper cases now have correlated terminal events, verified helper cleanup and original-volume restoration on native `37c190bbf14a` with unchanged frontend `3943b23cfb37`. This is controlled helper-path acceptance, not real-source playback qualification.

A controlled slow-helper request was accepted and failed with the startup-budget-exceeded reason. The helper process was confirmed gone and volume restored, but the trace had a resolution start without a matching terminal event. The outer command timeout dropped the nested resolver before its explicit end event. The original live attempt remains incomplete rather than a deadline qualification.

A resolution guard now retains the original operation identity and emits one end event on completion or future drop. Normal completion records its result; drop distinguishes cancellation, expired deadline and unrelated abandonment. The outer command already owns a pinned boxed resolver and now drops it before setting the timer-generated stop flag, preserving genuine cancellation priority without adding an allocation or await. Tests cover completion outcomes, outer expiry, cancellation, simultaneous ready cancellation/expiry and abandonment.

The same source revision adds trace-only aggregate candidate-gate evidence. Gate ordering and alias validation are preserved, and the existing lazy filter still stops at the foreground cap. Untraced playback does not construct or populate diagnostic maps. Counts describe the evaluated prefix, and raw duration deltas are explicitly descriptive rather than effective eligibility thresholds. No titles, URLs, queries, provider IDs or ISRC values are included. This instrumentation does not relax recording acceptance.

Independent static review passed. Private before-source snapshots, integration hashes, 541-input pin and native logs are under `.cache/qualification-resolution-terminal-trace/`. Compilation, tests, executable build and fresh live verification must pass before this is considered qualified.

The first native run compiles and ends with1,059 passed, one failed test assertion and21 ignored. The failure is a newly added diagnostic path assertion in the existing verified-alias Topic fixture: the first recording predicate already accepts through its internal Topic-metadata path, so the observed winning outer predicate is `RecordingVerifiedAlias`, not the later `TopicVerifiedAlias` leg. All original acceptance assertions and terminal-trace regressions pass. A test-only correction and explicit outer-predicate label documentation are being reviewed; the failed run is retained. No rebuilt live timing result is claimed.

The test-only correction is integrated, with no production acceptance change. The full native rerun passes1,060 tests, zero failures and21 ignored in18.39 seconds. All541 pinned inputs remain unchanged. The initial failed assertion and its source snapshot remain retained. Executable build and fresh live deadline/cancellation qualification are still pending.

Executable build passed in129 seconds, with all541 pinned source inputs unchanged. The immutable binary and mounted frontend handshake are verified before the fresh controlled case.

### Current controlled live results

The rebuilt native `37c190bbf14a` emits the missing correlated terminal event. A60-second stalled search ends with deadline=true/cancelled=false at19.989 seconds; the exact helper PID is gone. The original runner conservatively withheld qualification/volume restoration because public playback snapshots omit queueRevision. A separately retained fresh postflight verified identical registry receipt, exact occurrence/track queue, idle state and the still-owned mute, then restored0.8 volume. The original inconclusive report remains unchanged.

Search cancellation is fully observed by the corrected runner: terminal cancelled at1.471 seconds, deadline=false, helper gone, failure history unchanged and original volume restored. The first nested attempt remained inconclusive because bare synthetic titles scored low confidence and never reached extraction. Correcting only the synthetic search-result title to the realistic artist-title form preserves all production gates. The fresh nested repeat observes eight seconds of search followed by stalled extraction, then deadline=true/cancelled=false at19.990 seconds from the original resolution start. Helper cleanup and volume restoration pass.

The fresh automatic slow-search repeat ends at 19.991 seconds with deadline=true/cancelled=false, helper gone, exact queue-occurrence ownership verified and volume restored. Extraction cancellation ends at 1.496 seconds with cancelled=true/deadline=false, unchanged failure history and helper cleanup. A three-candidate fixture makes exactly two foreground extraction attempts, then terminates at 204 milliseconds; all helper processes are gone. This cap check is limited to the configured single provider, not a cross-provider aggregate claim.

| Controlled case | Resolution elapsed | Observed result |
| --- | ---: | --- |
| Stalled search | 19.991 s | Shared deadline, helper cleaned |
| Search 8 s then stalled extraction | 19.990 s | Original deadline retained |
| Cancel search | 1.471 s | Cancellation, no failure-history change |
| Cancel extraction | 1.496 s | Cancellation, no failure-history change |
| Three candidates, failed extraction | 0.204 s | Two foreground attempts |

Each corrected report records `fixture-observed`, no evidence problems, original volume restored and terminal cleanup. Timings are individual resolution spans, not click-to-audio latency or p95. Original malformed fixtures, inconclusive reports and the initial failed test assertion remain retained.

These controlled helper-path checks on Linux debug WebView do not qualify real network/probe stalls, full audio startup, statistical p95, other providers/platforms, or independent simultaneous cache callers. Those acceptance gates remain open.
