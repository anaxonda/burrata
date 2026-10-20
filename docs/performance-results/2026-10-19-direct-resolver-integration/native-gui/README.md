# Native GUI direct-resolver check

One anonymous fresh-profile GUI session passed startup, five seconds of native mixer/playhead progression, active seek, paused seek and resume on the explicit Matthew source `uZGRATckoBM`. The feature-enabled app was built at `c626eb3b1eb00a1dc03a6b6d38951eef930501e9`, with coordinator-reported executable SHA-256 `b24e06540a8b335405c066cc0ec38e552a6bb9641afd4f032e34f77347804776`. The experiment runtime flag was `1`, the engine was native and volume was zero. This is muted device-path evidence, with no physical audibility claim.

The original [67-row probe](probe.json), [layout measurements](layout.json) and [PNG](layout.png) were copied byte-for-byte. The [manifest](manifest.json) records hashes and source scope. Only 18 allowlisted phase JSON objects were extracted into [playback-phases.jsonl](playback-phases.jsonl); the raw application log is not published. That trace contains the direct-resolution/access events and no helper-preparation or extraction start events.

| Check | Observed result |
| --- | --- |
| Startup | Source ready at 1,005 ms; first mixer audio at 1,242 ms; playhead reached 5.201 seconds |
| Active seek to 60 seconds | Reached 61.254 seconds; seek first audio 668 ms |
| Paused seek to 120 seconds | Position remained 120 seconds and paused |
| Resume | Reached 121.022 seconds; seek first audio 509 ms |
| Layout | 954 × 1036 viewport, 106.5 px footer, automated findings empty |

The PNG was visually reviewed: the footer fits, while the fresh-profile onboarding dialog obscures the main list. This does **not** establish full-list visual acceptance. Startup timings are the application's operation metrics, distinct from the earlier fresh-test-process decoder matrix; no matched speed comparison follows.

This single session does not qualify catalog matching, additional IDs, natural next preparation, full recordings, recovery, long-duration resources, supported-platform packages or universal provider reliability. The source and earlier matrix provenance remain separate. [CI run 39357267865](https://github.com/anaxonda/burrata/actions/runs/39357267865) at `c626eb3` is terminal with all three platform jobs failed; the [retained failures](../ci-39357267865-failed-jobs.log) are CI/test-fixture issues detailed in the parent README. New [run 39358962075](https://github.com/anaxonda/burrata/actions/runs/39358962075) targets the CI/test-only corrections at `b6b876c` and is running; no corrected result or promotion is claimed. The restored app remains the runtime-equivalent `c626eb3` build. The [safe restoration report](restoration.json) confirms exactly one GUI, the preserved original profile and queue occurrence, restored volume, WebView engine, radio resumed and the experimental resolver disabled. The coordinator verified that this GUI runs the current `b24e` executable. Raw profile, queue and account state are not published.
