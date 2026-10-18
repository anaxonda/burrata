# Fruit Bats unpinned WebView resolution

One ordinary unpinned `fixture:fruit-default` request for Fruit Bats — The Bottom of It was tested on Linux debug native `7a43ea0c8def`, App entry `3943b23cfb37` and SearchPanel `980a63985977`. The fresh isolated profile explicitly selected Monochrome and disabled fallback in both persisted settings and its manifest. It used the default WebView engine, an owned silent sink and no experimental decoder/reserve flags. The normal user profile was stopped for the test; there was only one GUI.

The single queued operation resolved to `monochrome:239910648984977408`. The trace reports two search candidates, one canonical recording acceptance, one rejection, one extraction attempt and successful resolution at 1,666 ms. Candidate duration counters describe one within five seconds and one beyond five seconds; they do not independently establish why the rejected candidate failed. The media sample belongs to the accepted queue entry, session, source generation and seek serial. It reports a present stable source, duration 181.4 seconds, readyState 4, no media error, not paused and currentTime 0.159660 seconds.

This does not reproduce the fixture's no-playable-match failure on this build. It establishes source resolution/attachment and initial clock progress only. It does not qualify sustained output, acoustic identity, physical audibility, seeking, cold-start p95 or the original imported Spotify row, whose complete identity/version fields were not retained in this fixture. The separate native underflow remains open. No matching guard was relaxed.

The probe made no setup playback start. It submitted one queued UUID, received acceptance without receipt recovery, then paused only its owned occurrence, restored volume to 0.8 and cancelled that exact operation. Its total approximately eight-second wall time includes guarded MCP calls and cleanup, and is not first-audio latency. Private runner syntax checks and six offline contract assertions pass. During review, outdated media/status DTO assumptions and ambiguous mute cleanup were corrected before the live request.

The isolated app and owned sink were removed. The verified launcher restored the normal app and confirmed its mounted frontend. All 30 full queue objects, occurrence IDs/order, current index 0, paused position 3.287602 seconds, volume 0.8, repeat off and shuffle off exactly match the immediately preceding snapshot. No provider writes occurred.

Private evidence is retained under `.cache/qualification-fruit-unpinned-webview/`: the guarded profile receipt, queued result, resolver log, acceptance summary, fresh before/after snapshots, stop receipts and sink cleanup ledger. The runner source hashes are recorded in `acceptance-summary.json`. This is bounded diagnostic acceptance, not release qualification.

A read-only search of the restored Library found six matching catalog rows, including the exact canonical Monochrome studio row with the same title, artist, duration, ISRC and absent version as the successful fixture. The others include live variants and YouTube listings. No Spotify listing was returned in that bounded query, so it does not recover the original imported request or prove its failure cause. Playback was not changed by this read.

The subsequent read-only Kaval Monochrome query returns the exact choir/title catalog row at 131.706 seconds. Library membership remains 4,964 tracks, 42 playlists and 15 assets afterward. This is catalog metadata evidence, not Kaval playback acceptance.

## Three fresh-profile sustained explorations

Three further requests each used a separate fresh isolated profile and exactly one queued operation, without setup playback. Each resolved the same expected source and retained one session/source generation/seek serial throughout its healthy window.

| Run | Resolution | Healthy wall time | Media-clock advance | Sampled buffering/stalls |
| --- | ---: | ---: | ---: | ---: |
| 1 | 1,534 ms | 5.291 s | 5.246 s | 0 / 0 |
| 2 | 1,405 ms | 5.240 s | 5.185 s | 0 / 0 |
| 3 | 1,452 ms | 5.306 s | 5.256 s | 0 / 0 |

Every healthy sample required playing/unpaused state, readyState at least 3, no media error and a present stable source. The gate required five contiguous healthy seconds and at least 4.5 seconds of clock advance. Each run retained eight observation polls. Poll latency can miss shorter transients; zero observed buffering is not an event-complete absence claim. No transient was captured, so recovery remains untested.

All three runs paused only the owned occurrence and restored volume. The final isolated app and owned sink were removed; the mounted normal app restored all full queue objects, occurrence IDs/order and saved playback settings exactly.

Run 1's aggregate clock-delta field lagged one sample, while its healthy-window result was correct. Its raw report remains unchanged; the separate summary derives 5.246 seconds from its retained first/last timestamps. The one-line reporting fix was applied before runs 2 and 3. The original runner was reconstructed and verified against its pre-run SHA; both versions and support files are retained. The earlier launcher wrapper returned an expected error when its owned child was deliberately terminated; stop receipts verify the GUI and Vite were gone before the next launch.

Evidence: `.cache/qualification-fruit-unpinned-webview/sustained-exploratory-summary.json`, three raw reports, per-run resolver logs, hash-pinned runner copies, stop/cleanup receipts and fresh normal before/after snapshots. These are three exploratory Linux debug WebView clock-progress passes, not twenty measured repetitions, cold-machine startup, audible PCM proof, release-build performance or native Rodio underflow qualification.
