# Reader and seek deadline verification

The immutable development-test executable recorded in `native-manifest.json` passed the complete Linux host suite: **742 passed, zero failed, 14 ignored** in 11.74 seconds. Build warnings are retained in the log.

This checkpoint includes the absolute initial-reader deadline and early cancellation bridge, deterministic setup-future drop regression, actual loopback pending-header and stalled-prefetch cancellation, natural queue-end boundary regression, optional search automation coverage validation, and ready-seek timeout handling. Paused seeks wait until resume for a fresh deadline; generation plus watchdog epoch prevent a paused/resumed seek from being failed by an older timer. The asynchronous epoch regression uses a first-check handshake and coalesced pause/resume updates.

The separate YouTube reconnect-state changes were merged after this binary and are not covered by these counts. No replacement GUI was launched. First-mixer latency, whole-app resources, live recognition/providers and cross-platform qualification remain open.
