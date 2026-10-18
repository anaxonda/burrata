# Playback performance qualification

Native seeks now retain their decoder and downloaded ranges for supported formats, and the resolver prepares the preferred provider and first enabled fallback concurrently. The preferred provider wins during a configurable grace period (default 1500 ms); an already verified fallback can win when the grace expires or the preferred attempt fails. These changes have deterministic regression coverage. Production startup latency and CPU targets remain unqualified.

## Implemented behavior

- `streamingFallbackDelayMs` persists in settings, defaults to 1500 for existing profiles, and accepts 0–10000 ms. Playback settings expose the value in milliseconds.
- At most two provider attempts participate in the foreground race. Remaining providers run only after both initial attempts fail. A fallback failure never cancels a viable preferred attempt. Preferred results win ties.
- Existing pinning rules for selected candidates, direct provider tracks, chapters and playlist URLs remain in the serial resolver. Local files and radio bypass the race. Recording identity validation and transient access retries remain active.
- Request cancellation propagates to both attempts. Completion signals their shared cancellation channel and drops unfinished futures. yt-dlp subprocesses already use kill-on-drop. Background preloading still uses the same resolver and the existing playback cancellation lifecycle.
- Pointer scrubbing updates the range control locally and commits on pointer release. Keyboard scrubbing commits after 150 ms without further input. Cancellation and component destruction clear pending input.
- The native output worker combines adjacent queued seeks and retains the active native decoder, reader, HTTP client and downloaded ranges when seeking. Only the output queue and resampler are replaced. One shared network runtime serves streaming sessions. Prepared tracks transfer their decoder session when activated. Paused replacement players start paused. A retired reader that remains busy for 100 ms has its download cancelled before rebuilding; superseded waiting workers exit without setup. FFmpeg/HLS fallback still restarts its process and does not receive this decoder reuse benefit.
- FFmpeg fallback uses an exit guard that kills and reaps its child on every exit path, including read failures and disconnected PCM queues. A regression test checks that an early exit leaves no zombie process. This fixes process cleanup; it does not remove FFmpeg startup CPU or the need to restart that fallback on seek.
- Preview URL invalidation advances a cache epoch under the cache lock. A lookup already running at invalidation may finish for its original caller, but cannot repopulate the cache with its stale result; a waiting fresh-stream retry must resolve again. The epoch is global and conservatively skips caching unrelated in-flight results when any source is invalidated.
- Native snapshots expose `diagnostics.sourceReadyMs` alongside `firstAudioMs`. Both are elapsed from the backend playback request, excluding renderer click and IPC transit. Source-ready includes lookup, extraction, validation and retries. First-audio means consumption by the mixer, excluding device hardware latency.
- For a playing seek, `diagnostics.seekFirstAudioMs` starts at validated seek dispatch and ends at first mixer-consumed audio; `seekOperationFirstAudioMs` starts earlier at controller receipt before the command mutex. For a paused seek both remain unset until audio resumes: the decoder timer is then armed at Resume, while the original receipt timer still includes the user’s paused interval. Neither includes device hardware latency. Do not compare those paused-seek receipt values with playing-seek latency or treat the resumed decoder timer as UI Resume latency. The deferred-seek tests explicitly retain the original seek receipt and operation identity.
- Adopting a prepared next track resets first-audio, source-ready, recovery and buffering diagnostics instead of carrying the previous track's counts forward. Its first-audio measurement begins at adoption, so it must not be interpreted as preload preparation time or a measured audible gap.

MCP-originated startup, select, seek and explicit advancement commands without a client ID receive a fresh `native-<UUID>` operation ID. Startup/transition IDs and seek IDs remain separate; source phases carry the corresponding ID. `commandSerializedBytes` and `seekCommandSerializedBytes` count the UTF-8 bytes of the reserialized controller command, rather than transport framing or on-wire IPC bytes.

## Reproducible benchmark runner

Run `python3 scripts/performance/benchmark.py /absolute/path/cases.json --output /tmp/burrata-results --repetitions 3`. Use at least 20 repetitions for qualification, with a suitable live-request interval (`--interval`, default two seconds). Order is randomized using a recorded command-line seed (`--seed`, default 42). Each operation has a timeout (`--timeout`, default 45 seconds).

The manifest is an array of cases:

```json
[
  {
    "id": "native-local-opus-cold",
    "mode": "cold",
    "command": ["/absolute/path/playback-adapter", "--fixture", "/absolute/path/fixture.opus"]
  }
]
```

A playback adapter is required; this runner is not an application automation adapter. It must drive the selected engine and return one JSON object on stdout:

```json
{"firstAudioMs":123,"decodedFrames":240000,"continuedPlaybackMs":5000,"correctMatch":true}
```

An adapter must derive these fields from actual playback, not extraction metadata. The runner assigns each attempt a UUID in `BURRATA_BENCH_OPERATION_ID`; an adapter may echo it as `operationId`. This correlates adapter output to the runner attempt only. Optional numeric measurements include `readerReadyMs`, `decoderReadyMs`, `searchMs`, `extractionMs`, `retries`, `bufferingCount`, `transferredBytes`, `appCpuSeconds`, `webviewCpuSeconds`, `helperCpuSeconds`, `peakRssBytes`, `readBytes`, `writeBytes`, `ipcRequestBytes`, and `ipcResponseBytes`. Numeric measurements are summarized independently; absent values are omitted. IPC byte counts must be measured by an adapter or app instrumentation and are not inferred from elapsed time or OS counters. A successful attempt requires decoded frames, five seconds of continued playback, a correct recording and a first-audio measurement. Adapter success alone is insufficient. The runner cannot independently verify adapter honesty.

Cold cases get a fresh scratch profile per attempt. Other modes retain a private directory per manifest case; warm uncached and replay setup belongs to the adapter. The runner supplies `BURRATA_BENCH_DATA_DIR`, isolated `XDG_CONFIG_HOME`, `XDG_DATA_HOME`, `XDG_CACHE_HOME`, and a scratch working directory. Adapters must honor these paths and must not attach to the running application or use its library, queue or credentials. XDG isolation does not automatically isolate macOS/Windows application storage or an OS credential vault.

`attempts.json` and `attempts.csv` retain failures, timeouts and protocol errors, plus a harness `operationId`. `summary.json` reports median, nearest-rank p95 and worst numeric observations with attempt and failure counts. Timing samples from failed attempts remain visible; missing timing data is excluded from quantiles, never converted to a zero. On Unix, `cpuSeconds` includes the adapter and its reaped descendants, not an already-running app/WebView or persistent external service. Adapters may supply separately measured process-group CPU, RSS, I/O and IPC byte counts. Output is an allowlist of numeric metrics, the operation ID, recording correctness and fixed error categories; URLs and stderr are excluded. Use non-sensitive case IDs.

The runner has statistics and end-to-end isolation/protocol tests, wired into CI. Live provider qualification remains separate.

## Asset-backed library timing

`python3 scripts/performance/library_page_matrix.py --binary /absolute/path/burrata --output /absolute/path/results --counts 1000 --fixture-mode asset-backed` measures exact MCP page reads, including initial projection backfill/fallback costs by default. `--projection-state ready` explicitly waits for all synthetic one-asset library projections before timing and records that wait separately; compare initial and ready runs separately. The compatibility MCP response does not expose sort-pending state, so an absent pending field is not proof of cache readiness. These tiny WAV fixtures qualify metadata ordering and pagination, not audio decoding, indexing, release builds or whole-app performance.

The launcher keeps its 20-second app-registration limit by default. For an explicitly labelled slow-startup investigation, `--startup-timeout-seconds 180` allows a bounded longer wait and records both the configured limit and observed handshake time. This does not change the remote playback deadline. Retain failures at the original limit; a retry on an already migrated profile is not a fresh-startup result.

`BURRATA_STARTUP_TRACE=1` adds finite setup-phase timings to the app log for database open, download backfill, recovery, asset grants, bridge setup and endpoint publication. Asset grants report counts and page counts without filenames. These clocks start at Tauri setup, excluding process creation; collect them alongside the runner’s full handshake wait. The flag is off by default and does not change startup work.

The [initial 1k before/after comparison](performance-results/2026-10-17-cached-mcp/comparison.json) retains early slow title reads rather than excluding them after the fact. Its seven scenarios passed all 140 reads and exact continuations; title p95 remained roughly unchanged despite faster later reads. The [subsequent playback reproduction](performance-results/2026-10-17-buffering-reproduction/fruit-exploratory-03.json) failed on its third exploratory attempt with actual exhausted PCM and an in-flight input read. Neither result establishes full performance or playback qualification.

## Native seek measurements

`python3 scripts/performance/seek_benchmark.py --output /tmp/burrata-seek-release` builds optimized tests, generates five-minute WAV/FLAC/MP3/AAC/Opus fixtures in a scratch directory and compares a fresh decoder against a retained decoder. Add `--profile debug` to measure the development build separately. There are 20 repetitions at each of 1%, 50% and 95% of the file per strategy; strategy order alternates each repetition. Raw JSON/CSV, per-position quantiles and provenance are saved to the output directory. No application library, credentials or output device are used.

[Debug results](performance-results/2026-10-15-seek-debug/summary.json) measure first decoded PCM and decode-worker thread CPU, including setup and cancellation after the first received block. They exclude the renderer, resolver, mixer and hardware; the thread CPU metric also excludes FFmpeg subprocess CPU. On Linux the benchmark also records reaped decoder-child CPU separately as `decoderChildCpuMs`; this includes FFmpeg and excludes unrelated processes. The fresh strategy reconstructs the decoder in the current build; it is not a historical-commit application comparison. Both strategies use the same dependencies and fixtures. Opus currently uses FFmpeg, so its thread CPU cannot represent decoding CPU and is omitted from the summary. Results on generated tones do not qualify recording matching or live streaming.

The [protected 725-test-binary recheck](performance-results/2026-10-16-native-725-decoder-recheck/README.md) passed the local WAV duration and 95%-seek-to-EOF correctness test. Its accompanying first-PCM timing rows overlapped another Cargo compilation and are explicitly confounded, not qualification, and not a CPU/latency comparison.

For the development build, aggregating the same 60 seeks per format and strategy gives:

| Format | Fresh decoder p95 | Retained decoder p95 |
| --- | ---: | ---: |
| WAV | 1.43 ms | 1.08 ms |
| FLAC | 7.59 ms | 4.59 ms |
| MP3 | 14.75 ms | 11.76 ms |
| AAC in M4A | 5.78 ms | 2.15 ms |
| Opus via FFmpeg | 97.03 ms | 98.49 ms |

The [optimized results](performance-results/2026-10-15-seek-release/summary.json) contain 60 seeks per codec per strategy (20 at each position). Aggregating those positions gives these p95 decoder-to-first-PCM measurements:

| Format | Fresh decoder p95 | Retained decoder p95 | Decode thread CPU p95 fresh to retained | FFmpeg child CPU p95 fresh to retained |
| --- | ---: | ---: | ---: | ---: |
| WAV | 0.263 ms | 0.225 ms | 0.360 to 0.282 ms | No child process |
| FLAC | 0.551 ms | 0.375 ms | 0.604 to 0.355 ms | No child process |
| MP3 | 1.960 ms | 1.230 ms | 2.015 to 1.217 ms | No child process |
| AAC in M4A | 0.985 ms | 0.354 ms | 1.110 to 0.374 ms | No child process |
| Opus via FFmpeg | 96.316 ms | 99.854 ms | Not used for decoder CPU | 92.615 to 99.221 ms |

Native decoding overhead is small in this optimized fixture test. The milliseconds saved here cannot explain or establish a fix for seconds-long application seeks. The HTTP tests establish a separate benefit: fully buffered seeks can reuse the reader with the original server gone, while unbuffered seeks request ranges and decode the correct target samples. Remote seek p95, actual audio-device latency and renderer CPU still need end-to-end measurement. The benchmark retains the current FFmpeg fallback, which remains about two orders of magnitude slower than native-supported codecs on these fixtures. Its separate child-CPU measurement makes the cost of restarting that process visible; native decoder retention does not reduce it.

## Retained Opus decoder feasibility

The checked-in native stack is pinned to `rodio = 0.22.2` and Symphonia `0.5.5` (`src-tauri/Cargo.toml`, `src-tauri/Cargo.lock`). The lockfile contains no Opus codec crate. The `symphonia-all` feature does not change that: Symphonia 0.5.5 marks Opus as unsupported, and its Opus codec entry is a placeholder rather than a decoder ([pinned Symphonia 0.5.5 source](https://github.com/pdeljanov/Symphonia/tree/v0.5.5)). Rodio 0.22.2 builds its decoder from `symphonia::default::get_codecs()` internally ([pinned Rodio source](https://github.com/RustAudio/rodio/blob/v0.22.2/src/decoder/symphonia.rs)); its public `DecoderBuilder` does not expose a custom codec registry. Therefore enabling another feature on the current Rodio decoder cannot provide retained Opus decoding.

A prototype looks feasible without replacing the output mixer: add a Symphonia 0.5-compatible third-party libopus adapter (the [adapter repository](https://github.com/aschey/symphonia-adapters) describes its Opus integration) and register that decoder with a custom Symphonia codec registry, then implement an Opus-specific source that owns and reuses its Ogg reader and decoder when seeking. That source can be fed to the existing mixer, while non-Opus formats remain on Rodio's decoder. This requires an explicit decoder path because Rodio's built-in registry is not injectable. The adapter release compatibility, native libopus packaging across Burrata targets, license and safety review, long-file seek correctness, and performance against FFmpeg still need validation; none is established by this investigation. Keep the current FFmpeg route as fallback until those checks and end-to-end playback qualification pass. No dependency or runtime behavior change is included here.

## Running development app profiling

`python3 scripts/performance/profile_process.py --pid APP_PID --seconds 30 --output /tmp/burrata-profile` samples Linux process and thread CPU, RSS, and storage IO counters for that process and its observed descendants, including WebView and FFmpeg helpers. Use repeated `--group app=PID --group webview=PID --group helper=PID` to label separate process trees, and `--operation-id UUID` to put a runner attempt ID on every sample. Supply roots that do not contain one another to avoid double counting. It reads no command arguments, environment, app database or credentials. CPU uses 100% per logical core. Process rows include their own threads; group rows aggregate process rows once. Group RSS sums resident sizes and may double-count shared pages. Storage IO is not network traffic. Tasks that start and exit between polls are missed, so use the decoder benchmark's reaped-child counters for short FFmpeg operations. Playback and seek commands now propagate operation IDs from renderer entry to native playback snapshots. Startup diagnostics expose renderer start time, click-to-IPC delay, serialized command size, IPC receipt, source-ready and first-audio phases; seek has its own operation ID and click-to-IPC timing. With `BURRATA_PLAYBACK_TRACE=1`, resolver logs include nested provider/search, source and stream DNS, extraction, probe/retry, cache/coalescing, identity and cancellation/deadline phases for the covered native paths. The direct Monochrome adapter does not yet expose equally granular phases. Process-profiler samples remain labeled by caller-supplied operation ID rather than automatically joined to a playback trace.

### Isolated Linux app memory run protocol and status

After the current debug binary is built and the root agent gives the go-ahead,
first run the 60-second app smoke in its own new output directory:
`python3 scripts/performance/run_isolated_memory.py --binary /tmp/burrata-sorting-target/debug/burrata --duration-seconds 60 --settle-seconds 0 --qualify-local --output docs/performance-results/2026-10-15-isolated-memory-smoke3`.
Only if that passes, run the 30-minute capture in a separate new directory:
`python3 scripts/performance/run_isolated_memory.py --binary /tmp/burrata-sorting-target/debug/burrata --duration-seconds 1800 --settle-seconds 120 --qualify-local --output docs/performance-results/2026-10-15-isolated-memory-30m`.
The optional local qualification stage runs three warmups, 20 local starts,
20 seeks at each of 1%, 50%, and 95%, plus a paused-seek check; any recorded
failure stops the run before the memory measurement. The driver refuses to
overwrite an existing output directory and launches only through
`scripts/dev/launch-current.sh --isolated-profile`. That launcher
accepts only a marked `burrata-launch.*` directory directly under `/tmp`,
checks that the binary and frontend inputs are current, verifies the transformed
Vite module and completes the authenticated MCP `get_app_status` handshake.
It removes the isolated profile on exit; all measurements are written outside
that profile. It allows other Burrata executable paths to remain running but
refuses a duplicate process of this exact verified binary. No existing process
is stopped.

The seed database contains 20,000 synthetic metadata-only listings and one
generated six-minute local WAV recording. The MCP driver sets volume to zero,
plays six queued copies of the generated recording, confirms the playback clock
advances, and checks the final 100-entry search page. It then performs a
bounded 100-row library search and playback-state read every 15 seconds. For
the full capture, the profiler starts after the app has mounted, playback starts,
and a two-minute settle period has elapsed; the smoke has no settle period.
The sample interval is one second over the measured window.

The app-data profile does not automatically isolate the default playlist scan
root. The memory runner creates `fixture/Music` beneath its marked
`/tmp/burrata-launch.*` profile and sets `BURRATA_ISOLATED_MUSIC_ROOT` to that
canonical path. The app accepts this override only with the exact profile marker
`burrata isolated memory run\n`; invalid values fail closed and do not fall back
to the real home directory. Ordinary launches without the override retain the
existing `~/Music` scan behavior.
It captures the app and observed descendants, with group CPU, RSS and storage
I/O in the result summary. The Vite development server is sampled separately
and excluded from app totals. If port 1420 is already occupied, the launcher
uses the verified live listener PID only after confirming its UID/start time;
the server is labeled shared, never stopped during cleanup, and its resources
may include unrelated work.
App and Vite logs are retained under the selected output directory after the
isolated profile is removed. The smoke-four and smoke-five measurements
predate the Music-root override, so both used a private app database while
still allowing reconciliation against the host's default `~/Music` contents.
The retained evidence does not establish whether host files were imported or
changed. Treat those runs as synthetic local playback with a host-library scan
confounder, not as a fully isolated catalog workload or a matched before/after
CPU comparison. They do not qualify performance on a real user library,
provider login, or remote audio path. The 20k catalog rows also do not represent
an asset-heavy library.
Five smoke attempts are retained. The first mounted the frontend but stopped
on Vite PID discovery; the second stopped before launch because the target was
older than `native_output.rs`; the third mounted the app but rejected the Vite
process identity. The fourth completed local qualification with zero failures:
3/3 warmups, 20/20 starts, 60/60 seeks and a passing paused-seek check. Startup
wall time medians/p95 were 101/195 ms for the play IPC call, 112.5/224 ms to
observe first audio (6,407 ms worst), and 303/556 ms to observe playhead
advance. Seek call medians/p95 were 7/23 ms, 119.5/166 ms to observe seek
audio, and 247.5/374 ms to observe playhead advance. Its profiler did not start
because port 1420 was already occupied. In smoke five, the shared Vite server
was tracked separately and the isolated app was sampled for 60 seconds across
57 one-second intervals. App-tree CPU was median/p95/worst 27.619/116.043/
222.912% of one logical core (32.91 CPU seconds); RSS was median/p95/peak
575.3/585.8/626.3 MB; storage I/O counters were zero. The workload remained
playing and completed four paged searches and four playback-state reads. Vite
was a shared pre-existing server (PID 910643), so its CPU/RSS cannot be
attributed only to this app; measured separately, its peak RSS was 434.8 MB.
These synthetic 60-second observations are not steady-state or provider
qualification. Artifacts are retained in
`performance-results/2026-10-15-isolated-memory-smoke/` through
`...-smoke5/`. The launcher now identifies the actual listener PID and labels
shared Vite usage separately. The 30-minute run remains pending a separate
go-ahead. The current target is a debug build. If another Burrata
instance owns MPRIS or the audio device, those shared system services can affect
playback behavior; this run does not claim an uncontended physical-device
baseline.

The separate [120-second native remote-playback resource sample](performance-results/2026-10-15-native-remote-playback-resources/metadata.json)
contains 111 group observations per root. Burrata group CPU was median/p95/peak
6.538/111.913/130.693% of one logical core; summing the one-second CPU samples
gives approximately 31.67 CPU seconds. Group RSS was median/p95/peak
514.3/540.4/549.6 MiB. The app group includes observed descendants; these data
do not separate WebView/helper usage. Process/group rows are the CPU totals;
thread rows are detail for the same CPU and must not be added again. The
separately labeled shared Vite root had 111 observations, about 0.009 CPU
seconds by sample summation and 498 MiB RSS. Vite was already running and its
usage cannot be attributed solely to Burrata. Storage counters are not network
traffic. This short capture neither qualifies the playback path nor substitutes
for the later 30-minute whole-app memory run.

The [running-app capture](performance-results/2026-10-15-running-debug-sample/metadata.json) is from the **older development app**, confirmed by the user. It is not a measurement of the updated code, and playback/seek activity was not controlled. Across the sampled intervals, Burrata averaged approximately 39.1% of one core, its WebView process 2.8%, and the observed FFmpeg child 0.7%. Burrata peaked at 113.1% in one interval. The playlist watcher averaged 26.8% and peaked at 103.5%; audio output averaged 5.0%. This identifies playlist reconciliation as a whole-app profiling lead without establishing that it caused a reported seek spike. A separate initial 15-second sample averaged 11.3% in Burrata alone, illustrating why a single uncontrolled window is not a stable playback baseline.

The current metadata-repair path now checks for URL-only placeholder tracks before reparsing playlist comments. Fully described collections skip that repeated parser work. URL placeholder repair, stable IDs, manual-field protection and duplicate repair remain covered by the existing sync tests. This optimization has not yet been qualified with an updated-app CPU capture; scans and mirror generation still run.

## Verification and remaining qualification

Linux verification covers the frontend integration, scrubber, native playback state, source scheduling, native decoding and seek queue behavior. Latest reported checks: default Rust suite 527 passed and 13 ignored; frontend suite 372 tests across 46 files; latest playback UI integration 141 passed; extension suite 67 passed; separate Spotify/YouTube worker suites 8/17 passed; MCP suite 4 passed; Svelte checks report zero errors and warnings; production frontend build passes. These deterministic checks do not establish provider playback or clean-device qualification. Unit tests omit the implicit real `~/Music` discovery root and use explicitly configured scratch roots. Resource copying for Rust verification uses `TAURI_CONFIG='{"bundle":{"resources":[]}}'` to avoid overwriting a busy bundled executable; shipping configuration is unchanged.

Qualification still requires adapters for native and historical/current WebView builds, exact engine and dependency revisions, equivalent fixtures and device settings, complete trace coverage across providers, automatically correlated process/thread samples, and the named-recording provider/seek matrix including failures and recording correctness. The synthetic library matrix described below exercises only Python SQLite/JSON filtering and hydration-shaped queries; it does not execute Rust Store code or the application and cannot qualify application performance. The completed local release memory checkpoint is reported below. Earlier native playback qualification attempts were rejected: first mixer-consumed audio arrived after 7,132 ms, exceeding the 5-second limit; a warm 20-start sequence produced fresh generations but reported 170.079 seconds for a source verified at 250 seconds, so qualification stopped before seeking and no seek measurements were collected. A later full-identity attempt reported 251 seconds against 245.48 seconds and was rejected by the identity check. A subsequent shared-profile default-WebView attempt timed out at the MCP client after 25 seconds and crashed with SIGSEGV (exit 139) at resolver start, before provider work. The redacted [crash summary](performance-results/2026-10-15-shared-webview-resolver/crash-summary.json) records a function-only stack through `tokio::time::timeout<playback_sources::resolve_inner future>`, `within_deadline`, `resolve`, `resolve_with_playback_budget`, and `resolve_playback_track`. Large unboxed-future stack exhaustion is suspected but not confirmed; there is no SQLite lock evidence. After boxing the resolver futures, the subsequent [default-WebView live check](performance-results/2026-10-15-shared-webview-boxed-resolver/live-playback.json) verified playhead advancement with a 250.079-second source. [Seek checks](performance-results/2026-10-15-shared-webview-boxed-resolver/stable-seek-status.json) verified zero and 200 seconds; near-end advancement buffered intermittently and remains unqualified. These muted checks do not measure physical audibility or establish startup p95. The exact native muxed file was independently verified at 250.079 seconds, reproducing Rodio’s incorrect 170.079-second duration. The source-validated FFmpeg routing fix for muxed MP4 passes the exact private-file late-seek-to-EOF regression test; one subsequent [live native correctness gate](performance-results/2026-10-15-native-muxed-ffmpeg/verification.json) reports 250 seconds and advancing FFmpeg playback. First mixer audio arrived at 7,489 ms, so startup still misses the five-second target. The subsequent [repeated debug run](performance-results/2026-10-15-native-muxed-ffmpeg/summary.json) completed three warmups, 20 starts and 60 seeks with zero failures, including a paused seek/resume check. Cached native startup median/p95/worst were 397.5/522/536 ms; seek first-audio p95 was 507/732/717 ms at 1%/50%/95% respectively. These are warm-source results for one recording, not cold-start or cross-provider qualification. Native duration correctness now passes this recording’s gate; the later embedded-frontend release and completed 30-minute local memory checkpoint are reported below. A metadata extraction benchmark cannot qualify playback.

## Synthetic library discovery scale matrix

`python3 scripts/benchmark_library_discovery.py --counts 1000 5000 20000 50000 --warmups 3 --repetitions 20 --json` compares the script's former hydrate-all search loop with SQL-filtered paging on in-memory synthetic SQLite rows. The saved [report](performance-results/2026-10-15-library-synthetic/summary.json) includes median, nearest-rank p95 and worst wall/benchmark-process CPU seconds, operation counts and parity failures. It completed with zero parity failures at each scale. At 50k rows, hydrate-all performed 50,000 hydration reads per search versus 50 for the paged query; median wall time was 0.774 s versus 0.275 s, and p95 was 0.867 s versus 0.320 s.

These are synthetic Python measurements, not a Rust application benchmark: they exclude renderer, IPC, filesystem, real library data, application/WebView/helper CPU, and steady-state memory. No app-level performance or memory conclusion should be drawn from this matrix. The completed 30-minute local release memory run is reported below; it does not turn this Python matrix into an application benchmark.

The next investigations, ordered by concrete code observations rather than measured speedup, are:

1. FFmpeg-backed formats (including Opus in the current decoder configuration) still reconstruct their process on seek. Measure a persistent/native codec path, child-process CPU and real remote ranges; native-supported formats now reuse the existing reader and decoder.
2. Flat searches in `acquisition.rs` initialize managed YouTube support. Compare deferring helper initialization, with equivalent extraction and real access verification.
3. Nested candidate and access retry budgets can accumulate; add a request-wide deadline with cancellation and visible timeout diagnostics.
4. Whole-library views, SQLite waits, artwork bursts, background jobs and per-sample atomics need profiles at 1k/5k/20k/50k tracks before ranking fixes by impact.

There is no measured conclusion yet about a Rust cutover regression, SoundCloud being faster than YouTube, or interchangeability of Samidy and Burrata's Monochrome backend. The pinned-source behavior, FFmpeg decoder fallback and serial provider implementation remain rollback paths; disabling streaming fallback bypasses the provider race. Public release qualification remains outstanding, including macOS, Windows, remote playback qualification and the specified p95/CPU targets.

### Live Spotify session evidence

On 2026-10-15 the user completed interactive Spotify sign-in and explicit
account confirmation in the isolated native WebView prototype. The login
window closed; sanitized app status reported Spotify connected at revision 1
with `native-webview-http`. A read-only playlist listing returned 48 items and
a continuation cursor in 2.438 seconds. The user reported a successful
playlist import without independent item-level verification. After restart,
the OS-keyring credential recovered without interactive sign-in when the
authenticated MCP session-start command was used. Confirmation now auto-enables
the background session and hides the Start action; the one restart recovery
does not establish long-running background behavior. The redacted [live session
record](performance-results/2026-10-15-native-provider-session/live-read-qualification.md)
contains these boundaries.

Bearer-header forwarding was corrected for the public REST owner-playlists
request, but its live request returned HTTP 429 with a 22-second retry hint.
After following that cooldown, one retry remained rate-limited with an
eight-second retry hint and requests stopped. More fundamentally, Spotify's
[February 2026 migration guide](https://developer.spotify.com/documentation/web-api/tutorials/february-2026-migration-guide)
and [changelog](https://developer.spotify.com/documentation/web-api/references/changes/february-2026)
mark `GET /users/{id}/playlists` removed with no replacement; `GET /me/playlists`
is the documented current-user route, not a way to list arbitrary users.
Spotify documents that affected Development Mode apps must remove other-user
playlist/profile fetches, while Extended Quota Mode apps are unaffected by the
change. This qualification did not establish which quota mode applies to a
different integration or private interfaces. A separate private profile-view
route had one bounded successful read; limitations are recorded in the
[implementation status](implementation-status.md). Thus the 429 is an
observed failure for the attempted request, but should not be described as a
temporary blockage on a supported public-profile API; public-profile browsing
via the developer Web API is unavailable for affected apps. The 67-test
extension/worker suite passes, but that does not establish live API
availability. An imported Spotify track
reached MCP-reported `playing` status, with position advancing by 5.48 seconds
and a reported duration of 246.36 seconds. The qualification launch did not
force `BURRATA_PLAYBACK_ENGINE=native`, and snapshots did not identify the
engine, so this is not qualified as native playback. No first-mixer-audio
evidence was captured. The same-track replay was idempotent in this WebView
provider-session check and did not restart the current track; that failure
artifact was retained. A separate native local sequence did produce fresh
generations, but its duration mismatch invalidated its seek qualification as
described above. The identified bundled `managedBgUtils`
issue now falls back to a system executable when available; a separate CLI
check decoded 0.5 seconds (96,000 PCM bytes) in 4.623 seconds. That CLI result
is not Burrata application playback. Provider playback, recording matching,
seek recovery, account switching, long-running background behavior, and the
20-attempt provider matrix remain unqualified.

## Provider research boundary

[YouTube.js documentation](https://ytjs.dev/guide/getting-started) describes session caching, session-bound PO tokens, player retrieval and the interpreter requirement for deciphering. A persistent Node experiment must include those costs and real decoded playback. YouTube.js 18.1.0 is pinned with its own lockfile under `scripts/performance/innertube`, outside production dependencies. `worker.mjs` is a persistent JSONL process using the bundled Node runtime, a private session/player-script cache, an eight-entry expiring extraction cache and FFmpeg PCM decoding. Search results are not automatically accepted as recording matches. A playback request accepts an explicit expected video ID; seek requests reuse the client and cached extraction. Invalidations discard denied/expired streams, and short PCM outputs fail rather than count as full success.

Install the isolated dependency with `npm ci --prefix scripts/performance/innertube --ignore-scripts`. Start the bundled Node executable with `scripts/performance/innertube/worker.mjs`, setting `BURRATA_BENCH_DATA_DIR` to a fresh scratch directory and `BURRATA_BENCH_CLIENT` to `WEB` or `MWEB` for the tested clients. Each input line is one JSON request:

```json
{"action":"search","query":"Artist Title"}
{"action":"play","videoId":"jNQXAC9IVRw","expectedVideoId":"jNQXAC9IVRw","seconds":5}
{"action":"seek","videoId":"jNQXAC9IVRw","expectedVideoId":"jNQXAC9IVRw","position":5,"seconds":5}
```

Optional anonymous session inputs are `BURRATA_BENCH_PO_TOKEN` and `BURRATA_BENCH_VISITOR_DATA`; automatic token generation is not implemented in this prototype. `BURRATA_BENCH_FFMPEG` selects the decoder executable. Output redacts stream URLs and error text and reports stages, first PCM, decoded audio duration, helper CPU and helper RSS. Helper CPU excludes the FFmpeg child. `decodedAudioMs` is audio decoded as fast as possible into a discarded PCM pipe, not time heard through an audio device; it does not qualify uninterrupted playback. Offline protocol/cache/PCM tests run in CI without provider requests or installing the experimental dependency.

[Live smoke results](performance-results/2026-10-15-innertube-smoke/mweb-attempts.json) cover one explicit public clip, with two seconds between requests. The WEB client searched successfully but failed four audio attempts at deciphering, ultimately reporting no usable stream URL. MWEB decoded five seconds successfully in an initial attempt and in a separate four-command cold/warm/seek sequence. In that sequence, cold preparation took about 5.24 seconds; warm replay and seek extraction reused the cached session and took roughly 0.8–1.1 ms. Warm decoder-to-first-PCM took roughly 382–549 ms, and helper RSS reached about 426 MB. The smoke ran alongside optimized compilation, so these timings are exploratory rather than comparable qualification numbers. All successes and failures are retained in the result directory.

This establishes a working persistent prototype and a client-specific failure, not an InnerTube performance win over yt-dlp. The ten-recording correctness/quality comparison, token lifecycle, actual native playback integration, CPU including decoder children, and at least 20 repetitions per important path remain outstanding. No replacement resolver is promoted; the 30 percent p95 improvement gate remains untested.

[Nuclear's streaming documentation](https://github.com/nukeop/nuclear/blob/master/packages/docs/plugins/streaming.md) separates candidate search from just-in-time audio URL resolution. That boundary is relevant to queue preparation, but does not establish an implementation speedup for Burrata. The following source review pins the inspected revisions. No reference-project code was copied and no new runtime dependency was introduced.

| Project and revision | Inspected behavior | Reusable idea and cost |
| --- | --- | --- |
| [Nuclear d885282](https://github.com/nukeop/nuclear/tree/d88528277271e5af61f18db9924c01363664ff0a), AGPL-3.0 | [Streaming API](https://github.com/nukeop/nuclear/blob/d88528277271e5af61f18db9924c01363664ff0a/packages/docs/plugins/streaming.md) separates candidate discovery, URL resolution and expiry refresh. | Candidate preparation can outlive an expiring URL. Burrata already has these boundaries; adapt expiry tests rather than importing a runtime or code. Any code reuse requires license review. |
| [pear-desktop fb3923c](https://github.com/pear-devs/pear-desktop/tree/fb3923ce4d9ec2998826e51d08337be334eacd84), MIT | [Renderer](https://github.com/pear-devs/pear-desktop/blob/fb3923ce4d9ec2998826e51d08337be334eacd84/src/renderer.ts) sends seek and play commands to the existing embedded music-player API. | Browser playback retains its existing player/session. This supports investigating session retention, but cannot establish a Rust decoding speedup. Adopting its browser runtime would be a substantial architectural change. Its downloader was not used to infer playback behavior. |
| [Meld e0af269](https://github.com/FrancescoGrazioso/Meld/tree/e0af26953c315e6d8dfcef25b6436f7747fb34c6), GPL-3.0 | [Stream URL cache](https://github.com/FrancescoGrazioso/Meld/blob/e0af26953c315e6d8dfcef25b6436f7747fb34c6/app/src/main/kotlin/com/metrolist/music/playback/StreamUrlCache.kt) tracks expiry, request headers and generation-aware invalidation; its Spotify/YouTube mapper keeps successful matches in memory and Room. | Preserve verified recording IDs separately from signed URLs and prevent stale in-flight writes after invalidation. Burrata now has a regression test for the latter. Porting Android Media3/Room itself would add high maintenance cost; no such dependency is proposed. |
| [Monochrome a3c86df](https://github.com/monochrome-music/monochrome/tree/a3c86df0436a3eedc341ab5163fc5ee6b1688b5d), Apache-2.0 | [Tracks API](https://github.com/monochrome-music/monochrome/blob/a3c86df0436a3eedc341ab5163fc5ee6b1688b5d/js/tracks-api.js) uses `tracks.monochrome.st`, `/search/tracks` and `/track/{id}`, matching Burrata's public adapter. [Unified API](https://github.com/monochrome-music/monochrome/blob/a3c86df0436a3eedc341ab5163fc5ee6b1688b5d/js/api.js) separately handles bearer tokens, Turnstile JWTs and `/api/v2/track/`. | Public Tracks and unified playback are distinct integrations. Keep actual codec/access verification; frontend FLAC labels are not audio validation. Hostname substitution is not justified. |

A fetch of `monochrome.samidy.com` returned HTTP 502 in this review, so deployed frontend/backend requests could not be verified. The upstream source revision does not establish what Samidy currently deploys. The outdated-instance warning remains relevant. No Samidy integration or new credentials were added.

### Completed isolated release memory checkpoint (2026-10-15)

The [30-minute report](performance-results/2026-10-15-isolated-memory-30m-release/summary.json) completed with exit 0 using immutable release SHA `cc3e982f4a054c19428854e4de14d5b3f7da3fdff0c5b84826522932882dc639` and an embedded frontend, with no Vite process. The isolated 20,001-row synthetic library passed three warmups, 20 starts and 60 seeks before measurement. All 119 periodic playback observations advanced, with five sequential queue transitions and no observed failures.

Across 1,720 process-tree intervals, CPU median/p95/worst were 12.460/23.619/40.418% of one logical core, totaling 244.47 CPU seconds. Summed process RSS median/p95/peak were 542,044,160 / 547,717,120 / 548,900,864 bytes (about 517 / 522 / 523 MiB). RSS can double-count shared pages; storage reads were 98,304 bytes and storage writes zero, which does not measure network traffic. This local Linux fixture does not qualify remote providers, physical audibility, a historical Rust/WebView comparison or other platforms. Subsequent sorting, cancellation and UI instrumentation edits postdate this immutable binary and require separate verification.

The [five-minute window comparison](performance-results/2026-10-15-isolated-memory-30m-release/rss-windows.json) has median summed RSS of 546,156,544 bytes in the first window and 542,101,504 in the last, a decrease of 4,055,040 bytes. This run shows no increase between those windows; a single synthetic run does not establish absence of leaks.

### Monochrome-first native Poso qualification

The current debug build (`68dfb9bfc202b28ba2b40b69625109e7614f6482a5961bb6346062b9396e5734`)
passed an isolated, muted native/Rodio Monochrome gate for the exact public
Spotify recording at 245.48 seconds: source-ready 1,406 ms, first mixer audio
2,613 ms. The [clean repetition](performance-results/2026-10-15-monochrome-primary-native/repeated-qualification-clean-escalated.json)
passed three warmups, 20 starts, 60 seeks and paused seek/resume with no failures.
Warm startup-to-observed-first-audio median/p95/worst were 693/922/942 ms;
seek-to-first-audio were 118/126/132 ms. Seek IPC median/p95/worst were
13.5/19/24 ms. The paused seek stayed paused and resumed with advancing audio.

These are one-recording, warm-source results on Linux. The first gate is one
cold observation, not cold-start p95. The separate YouTube muxed source has a
250.079-second duration and its earlier 7,489 ms cold start still misses the
five-second target. No engine speedup, steady-playback CPU comparison, Opus
promotion, fallback coverage or broader provider reliability follows from this
single-provider run. The prior simultaneous-Sasa run is retained as contended;
a rejected extra MCP argument and an unprivileged loopback connection failure
are retained as harness/environment failures, not provider failures.

## Qualification snapshot diagnostics follow-up (2026-10-16)

The local playback qualification harness now retains startup/seek click-to-IPC timing, serialized controller-command sizes and seek controller-receipt-to-first-audio timing from existing native diagnostics. Values remain null when unavailable. Serialized command sizes measure the controller input JSON; they are not transport framing, response bytes or complete IPC traffic. MCP-generated requests without renderer timestamps do not establish user click latency. `node --check scripts/performance/local_playback_qualification.mjs` passes. No playback run was performed for this reporting-only change, and prior artifacts have not been rewritten to imply they include these fields.

## Explicit imported-playback matrix

`scripts/performance/imported_playback_matrix.py MANIFEST --output NEW_DIRECTORY` runs the checked-in native qualifier for each explicit case against an already-running isolated app. The JSON manifest uses `schemaVersion: 1`, `automationFile`, `binary`, `binarySha256`, `version`, and `cases`. Each case provides `id`, saved `trackId`, `expectedRecordingId` (the candidate source ID), `expectedProvider` (`monochrome`, `youtube`, or `soundcloud`) and independently verified `expectedDurationSeconds`. No commands come from the manifest. The app must already have a single-track native queue; each case mutes before selecting its target. The required marker and discovery path keep this out of the main profile.

The runner pins the supplied executable file hash, but does not independently identify the running GUI executable. Establish that match through the launcher/process manifest before using timings as build comparisons. Cases retain their own output and incremental matrix summary; source identity is checked after readiness, including wrong or absent observations. Matching candidate IDs and duration does not prove acoustic content. Three warmups, twenty startup observations and sixty seeks are attempted only while setup and prior samples succeed; a failed/incomplete case is not a qualified twenty-sample path. No live run of this new matrix has been completed. Eight orchestration fixtures and four manifest/report tests pass; see the current-main integration archive.

## Magalenha WebView diagnostic checkpoint —2026-10-17

Candidate7c8c46e64110 has two fresh isolated cold-start clock-progress successes, with ready-source observations8.970s and8.725s. The V4 source resolver takes6.796s; V3 measured search2.091s, extraction4.424s and parallel range validation at most294ms. These are debug single observations, not p95 or a five-second startup-target pass. V4 proves95% seek target landing and advancing media time but retains readyState2/buffering until end; seek-ready qualification remains failed. No physical-audibility, acoustic-identity or native-mixer claim follows from WebView media time. See the [detailed evidence](performance-results/2026-10-17-magalenha-source-review/README.md).

Native qualification reports now retain fixed-field starvation stream counters and paused-seek before/after/resume snapshots. Missing output-ready/pending-generation internals remain explicitly unavailable; binary and source hashes are recorded independently with an unverified-binding label unless separately proven by root's build/process manifest. Two telemetry allowlist/unknown-value tests and runner syntax checks pass. Prior Fruit reports are not retroactively upgraded.
