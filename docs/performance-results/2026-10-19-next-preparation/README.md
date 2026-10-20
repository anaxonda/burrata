# Queue preparation experiment

The first exploratory invocation did not reach playback. All six planned arms were rejected by the launcher native-source freshness guard; cleanup completed. The original driver reported only `launcher_failed` and removed its disposable logs. A subsequent read-only execution of the launcher prefix reproduced the stale-binary rejection before any Vite/app launch. The binary predates five changed Rust sources, including test-only research modules and their bridges.

The [original report](initial-stale-build-failure.json) is retained unchanged (SHA-256 `bd15dc680cdd44035479d075e9438e27f685b2143c0f49baca319b3e60d3e66b`). These are setup failures, not source, transition or CPU measurements. The harness is being changed to retain typed setup reasons and leave later attempts unstarted after a deterministic setup failure. A freshly built executable is required before a separate run; the launcher gate will not be bypassed.
