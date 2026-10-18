# Local WebView seek control — debug Linux

The generated60-second stereo48kHz WAV control reproduces the post-seek readiness symptom without a provider request. This narrows the problem beyond YouTube extraction or signed-URL expiry alone; it does not establish a WebKit, decoder, HTTP-server or hardware-output cause.

On candidate `dd9474d3fdf9`, the exact local WAV sustained5.005s of owned media-clock advancement over5.033s, across eight observations at readyState4. It then sought to48s. Bounded events record `seeking` at48 with readyState4 and buffered0–60, followed by `waiting`/`seeked` at readyState2 and `stalled` at51.493s. Clock movement continued to60; readiness returned to4 while paused at the end. No media error was reported. The strict five-second post-seek readiness gate failed; the runner ended at its phase deadline. Clock movement and full buffered ranges are not decoded-output or physical-audibility proof. This is a debug correctness control, not a startup/seek latency benchmark or p95.

A separate paused seek from the ended source emitted a48s seeking event, but subsequent owned media snapshots stayed at0s, paused, readyState4, with no seeked event. It did not land within tolerance before the25s work bound, so resume was never issued. This does not test or disprove a pause/seek/resume workaround from a still-playing source.

The manifest also contains hash/ffprobe-verified AAC/M4A and Opus/WebM controls. They were not played because the driver stopped after the WAV seek failure. No all-codec comparison is claimed. Earlier offscreen WebKit attempts failed before loading media, and the first actual-app harness attempts exposed a profile-parent calculation error and premature media inspection; those failed records remain retained rather than counted as codec failures.

Evidence: `.cache/qualification-local-codecs/result-v2-private.json`, `paused-seek-resume-private.json`, `manifest-private.json`, and pinned runner inputs. Both executed protocols verified paused cleanup and volume restoration. The isolated window was closed, then the sole normal app was restored with its fresh100-entry queue, occurrence IDs, selected index2, paused position3.199333s, volume and repeat/shuffle settings unchanged. Transient media-session/decoder fields are not compared across process restarts.


Subsequent isolated-output work on candidate `ecc2c40a10e8` captured continuous PCM during an owned readyState2/buffering reproduction. See [the output controls](../2026-10-17-isolated-seek-output/README.md). This provides evidence absent from the muted runs above; it does not retroactively prove their output or qualify all codecs/recordings.
