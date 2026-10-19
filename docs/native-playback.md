# Native playback qualification

Current failure instrumentation: `diagnostics.lastBuffering` retains finite reader/download/PCM facts from an actual empty PCM queue, tagged with generation and seek serial. It clears at new source/track, seek and preload-adoption boundaries; stale events do not replace it. Reader read/seek wait times and time since download progress expose input waiting alongside an empty production queue, but cannot independently separate network, storage and scheduling causes. `readInFlightMs`/`maxReadMs` and `seekInFlightMs`/`maxSeekMs` wrap the actual delegated operations; a null read wait does not exclude a blocked seek. Progress is reported after chunk storage writes, so a lack of progress alone does not prove a stalled network. `downloadedRangeEnd` is a range position, not cumulative network bytes. Existing decoder `bytesRead`/`bytesReceived` remain startup snapshots, rather than live throughput totals. The instrumentation does not change queue capacity, prefetch, fallback, cancellation or five-second verification gates.


Native playback is implemented and in qualification. Keep it behind the internal `playbackEngine.v1` preference (`webview` by default) until the matrix below passes on clean macOS, Windows, and Linux machines. The build smoke override `BURRATA_PLAYBACK_ENGINE=native|webview` is internal and is not a public settings preview. Enable native playback by default only after the results have been reviewed. A manual external-player action remains available as a diagnostic fallback in the WebView UI.

The implemented stack uses Rodio with Symphonia for decoding and output, plus `stream-download` for remote audio streams. FFmpeg is the fallback for HLS and HE-AAC streams that the native path cannot decode reliably. Qualification covers start, pause/resume, seek where supported, source retry, track changes, and gapless transitions. Crossfade, equalizer, and loudness normalization are deferred. The playback path does not depend on libmpv or a supervised media server/daemon.

## Build and automated checks

Native CI must build each target with the qualification path enabled and run its deterministic decode test. The contract script creates a one-second 48 kHz mono PCM WAV fixture locally and passes its path to `native_output::tests::decodes_generated_wav_without_audio_device` through `BURRATA_NATIVE_PLAYBACK_FIXTURE`; the test decodes and consumes samples without opening an audio output device. This is a build/decode check, not a claim that a physical device was exercised. Platform builds and manual qualification are still pending review.

Run the focused contract locally with:

```sh
python3 scripts/release/test-native-playback-contract.py
cargo test --locked --manifest-path src-tauri/Cargo.toml
```

The release workflow should run the focused contract on each native builder along with the full Rust tests. A passing CI result does not replace the manual device and stream qualification below.

## Manual acceptance matrix

Run on a clean supported OS install with no development media tools on `PATH`. Record OS build, device model/driver, Burrata commit, selected source, and result for every case. Test both built-in and USB/Bluetooth output when available, and verify output selection and volume behavior.

| Case | Required observation |
| --- | --- |
| Afeto — `4-XSYttqtls` | Resolve and start the exact recording; verify audible output, pause/resume, seek when offered, and normal end-of-track behavior. |
| Fred S — `1998651815` | Resolve and play the exact track; verify metadata, output, seek, and stop/cancel during loading. |
| Enough — Discogs `36487582` | Resolve the exact Discogs release track and play it through the expected source path; verify track changes do not leave old audio active. |
| Radio Paradise | Start the live station, play continuously for at least 30 minutes, and observe metadata changes, reconnect/retry behavior, and stable memory use. |
| North 103.3 | Start the station and play continuously for at least 30 minutes; verify recovery after a dropped connection and normal stop/restart. |

For each case, capture at least three cold starts and record time from Play to first audible audio, retry count and elapsed retry time, peak and steady-state process memory, and output device. Confirm expected behavior across pause/resume, rapid next/previous, source failure, reconnect, and application sleep/wake where applicable.

Mandatory reliability checks on every OS include physically removing the active output device during playback, reconnecting it, and verifying recovery or a clear actionable error; and suspending/sleeping the machine during playback, waking it, and verifying recovery or a clear actionable error. Also repeat device removal while paused and while a stream is resolving. Do not infer these outcomes from CI or from another operating system.

For HLS and HE-AAC sources, verify native decode first and then the FFmpeg fallback. Record which path handled the stream, startup latency, retries, audible continuity, and errors. Validate that unsupported or failed sources stop cleanly without leaving the queue or player state stuck.

Compare native playback resource use and packaging impact with the pre-change baseline on the same runner and artifact format. Record installer size and installed application size, plus startup-to-first-audio, retries, and process memory under the same track and device conditions. Keep the measured baseline and qualification results with the internal release evidence. Do not claim that tests passed until the platform-specific runs are complete.

## Release status

Promotion of native playback to the default remains on hold until clean-machine qualification is complete on macOS, Windows, and Linux, including device removal and sleep/wake. Public v0.1.3 uses WebView playback by default; its publication does not qualify the optional native engine. Publishing a new release is outside this qualification document.

## Local Linux evidence — 2026-10-14

The development desktop build was exercised with `BURRATA_PLAYBACK_ENGINE=native` on Linux with PipeWire. The Rust library suite passed 387 tests (10 ignored), the frontend suite passed 254 tests, Svelte reported no diagnostics, and the deterministic generated-WAV qualification script passed. The standalone custom-protocol debug build succeeds. These are local development results, not clean-install or cross-platform certification.

MCP drove real playback of Afeto (`4-XSYttqtls`), the Fred Again SoundCloud piano set (`1998651815`), Radio Paradise, The North 103.3, and existing downloaded Opus/MP3 files. Playback clocks advanced; pause/resume and seeking were exercised. A queue containing downloaded Ha Ya and Mi Gente transitioned to its preloaded next entry. MPRIS reported the native player's status and Radio Paradise's cached artwork as a file URI. Radio Paradise supplied current-song metadata and listening-history entries. The North's native PCM stream produced a SongRec fingerprint match for Rockstar by André Cymone; the station slogan no longer blocked recognition. The fingerprint result remains an unverified catalog identity.

Single observed starts were approximately 50–160 ms for downloaded tracks, 1.2 seconds for Radio Paradise, 2.8 seconds for The North, 5.6 seconds for the SoundCloud set, and 11–16 seconds for Afeto. An MP3 seek completed in approximately 250 ms. A remote Afeto seek completed in approximately 9 seconds after adding decoder length/seekability hints; this still needs improvement through stream/reader reuse. These observations are not comparative benchmarks.

YouTube returned intermittent preflight HTTP 403 responses during repeated tests. Resolution now permits one uncached alternate-format retry for the exact same video; it does not change a pinned recording. Catalog searches retain their bounded matching-provider fallback. CDN refusal must remain visible if fresh recovery fails.

Still required: clean packaged installs, macOS/Windows builds and physical playback, the full 30-minute station runs, device removal, sleep/wake, repeatable cold-start measurements, and gapless audio-output measurements. Native playback remains gated; these checks concern native-engine promotion, not the already published WebView-default v0.1.3 release.

Responsive UI checks used Chrome at 960px and 800px, with the right player pane open and closed. The shared radio component stays within its pane, its history scrolls, and list actions remain visible. Main-library virtualized rows remain 60px high while titles wrap. At desktop half-width (960px), opening the player pane retains the resizable sidebar. Below 901px the sidebar uses the compact rail; both information and queue panes have resize controls where space permits. Selecting the already-current station is navigation only.

The broader layout audit isolated radio/search component classes from global footer and empty-state rules. Long station names, artist and release names, context menus, queue controls, settings connections and tool setup were checked at constrained pane widths. Radio idle placeholders no longer inherit a 260px empty-library minimum height, and playing-info artist buttons override global no-wrap rules. The docked queue now overrides its standalone popup positioning and stays inside the right-hand pane. Browser fixtures use mocked Tauri data; they verify rendering, not live provider behavior.

Library sizing follow-up: the internal workspace uses a single flex child with explicit full width; details remain in the outer player pane/overlay. Chrome measurements at 800px matched list/table width to the main pane both with info open (416px) and closed (736px), and at 1440px the list filled the 1230px main pane. A later 4,500-track fixture reproduced the reported gap: virtual spacer rows still spanned six columns after responsive CSS hid two headers. Giving the spacer cells the same responsive column classes removes the phantom columns. At 960px, the last visible column reaches the table edge with the player pane open and closed.

Output recovery follow-up: CPAL buffer-underrun notifications no longer force a paused/disconnected state. Device-unavailable, invalidated-stream, and backend errors use bounded sink-reopen attempts while preserving the sample-derived playhead and intentional pause state. Unit tests cover error classification and recovery calculations; physical device-removal qualification is still required.

Latency and queue follow-up: native playback no longer runs the legacy first-three-library-track preview warmups alongside actual queue preloading, including during startup. A rejected signed audio URL is evicted before fresh extraction; transient access checks use a bounded retry before requiring another extraction. Known-size direct audio also receives concurrent bounded beginning/end range validation to reject truncated responses before decoding.

Initial source resolution owns its bounded retries; the session recovery loop applies after audio has actually started. A multi-item queue can skip up to three consecutive track-specific unavailable/denied/no-match failures, while global network/output errors stop with an error. Explicit retry after failure resets that attempt's timing and recovery budget. End-of-track checks use decoder-confirmed durations or chapter bounds rather than treating catalog estimates as exact, and the worker flushes its final sample position before reporting completion.

Before these changes, one observed retry took 9.86 seconds. Sampling confirmed CPU bursts in Burrata and its yt-dlp/JavaScript helpers during extraction, reaching about 1.8 logical cores combined during a failed queue transition; idle GUI use was about 5% of one core. These are development observations, not a repeatable performance benchmark or a measured speedup. InnerTube work remains deferred.

A subsequent per-thread trace also found the playlist watcher consuming 44% of one core on average over twelve seconds, with a near-full-core peak. Mirror-directory settings are now cached between refreshes, event draining is bounded, unchanged watcher results no longer reload the GUI, and fallback reconciliation runs every sixty seconds instead of fifteen. Unedited mirrors are checked against their saved file hash before loading their full catalog tracks. Real filesystem edits still trigger debounced reconciliation. The complete Rust suite covering the watcher and mirror changes passed 429 tests (10 ignored).

Large-queue follow-up: an actual 4,535-track playback-state request was 14,050,453 bytes and took 3.39 seconds. State events now carry a queue revision and omit unchanged queue arrays. Queue persistence is separate from changing session position, and MCP reads use bounded pages. The frontend retains its queue across state-only updates and selects an existing queue entry instead of resending the whole library when its playback inputs are unchanged.

Full yt-dlp extraction now supplies chapter metadata, including a known-empty result, so playback does not start another extraction just to check chapters. Default Original Mix labels no longer clutter search queries. A low-confidence Topic result is only provisionally considered when title and duration match; the same audio extraction must then confirm the full artist credit and recording identity before playback. Download matching remains strict.

The exact No Viviré source demonstrated a transient HTTP 403 on a fresh signed URL, then a successful range request on the same URL after a delay. Preflight therefore allows one cancellable 1.5-second retry before requiring another extraction. A one-second AAC decode succeeded from its returned range bytes. This does not guarantee access to every YouTube URL.

Applied-build verification: the same 4,535-entry queue returned a default 50-entry MCP page in 82–84 ms (140,463 bytes); its final page correctly contained 35 entries. Real native playback resolved Make Me Sway to `fsT7iQxn1m4` at 262.86 seconds. A near-end seek exercised stream recovery before the queue advanced to No Viviré (`LSHM_bkSYQI`), which remained playing after 71 seconds with AAC 44.1 kHz stereo at about 130 kbps. The original 4,535-track queue and paused selection were then restored and verified. Cold Make Me Sway startup still took 20.7 seconds in this run, so the queue/IPC improvement is not a claim of uniformly fast cold YouTube startup. The final scoped backend suite passed 424 tests (10 ignored, unchanged playlist-sync cases already covered by the full run), and all 302 frontend tests passed with no Svelte diagnostics.
