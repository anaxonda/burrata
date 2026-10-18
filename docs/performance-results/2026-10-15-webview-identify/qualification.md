# Default-player recognition live check

The isolated debug app passed its authenticated frontend handshake with 70 MCP
tools. Binary SHA-256:
`ced6cc73fd2c5b7205ba6650e10edab1d95f0113487da7b73dd2e450b5be6394`.
Its previously confirmed Spotify account recovered from the OS keyring without
another sign-in. This does not qualify account switching or YouTube login.

A muted MCP playback check selected the public imported Poso recording, sought
to 30 seconds, and observed the playhead advance with a 250.079-second duration.
The snapshot included the resolver's source generation. The no-argument
`identify_playing_track` command then returned a title matching Poso and a
nonempty artist in 1,746 ms. The sanitized numeric/boolean result is in
[live-check.json](live-check.json). The returned artist was not independently
compared, so this is not a complete recording-identity qualification. Physical
audibility was not measured. The app was left paused with its original volume
restored; no provider playlist mutations were performed.

Recognition uses the registered, validated source and current absolute position;
it returns a separate result without a tag-write command. Stale track, source,
seek and session checks have deterministic coverage. This single live run does
not qualify expired sources, long concerts, chapters, cancellation during
recognition, other codecs, or repeated latency.

The frontend now applies the automatic source-resolution deadline's remaining
time to stream attachment, resume seeking and `play()`. Lazy operations are not
started after expiry. Failure cancels the attempt; late playback completion
cannot pause a newer source. The HLS source identity remains valid after a
successful startup. The focused App integration suite passes 141 tests and
Svelte reports zero diagnostics. The full frontend suite immediately before the
last focused regressions passed 363 tests across 46 files; these counts have
different scopes and should not be combined.

The exact run harness is retained as [check.mjs](check.mjs). From the repository
root, run `node docs/performance-results/2026-10-15-webview-identify/check.mjs
AUTOMATION_FILE OUTPUT_FILE` against the explicitly isolated verification app.
The harness is tied to this public recording and recorded debug SHA; it is a
correctness excerpt check, not a general startup benchmark. It pauses at exit
and restores volume, but does not restore the prior queue.

## Current-audio audit follow-up — 2026-10-16

The native player recognizes a bounded, generation-tagged ring of consumed PCM. The WebView music path instead decodes an excerpt from the registered validated source at the current absolute position, with track/source/seek/session guards. These are different capture methods; neither the single Poso run above nor source inspection proves every audible-output case.

The WebView radio path still opens the station URL again through FFmpeg. A fresh live-stream connection can sample a different broadcast moment than the active media element. This remains an implementation gap for identifying the audio actually being heard; it must not be counted as completed current-audio capture. A real media-element PCM bridge would need CORS/browser capability checks, a bounded capture, cancellation, source/session identity checks and silence detection. Silently switching the default playback engine is outside this fix.

The frontend now rejects a delayed radio result or error after playback session, track, seek serial, source generation or native generation changes, including leaving and returning to the same station. The actual App integration suite passes 162 tests with this regression. This guard does not repair the fresh-stream capture limitation. Live ordinary-track, long concert, chapter-boundary, track-switch and radio capture checks remain open.
