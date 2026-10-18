# Local app playback qualification

The current Linux debug executable mounted its Vite frontend and passed the
authenticated MCP handshake in a private app profile containing 20,000
synthetic catalog listings and one generated six-minute WAV asset. Volume was
zero; playhead advancement and native mixer-consumed first-audio diagnostics
were checked. No production profile or provider account was used. However, the
launcher did not redirect the application's default `~/Music` discovery root.
The app could therefore reconcile the host Music directory in its private app
database during this run. This is not a fully isolated catalog workload, and
the results cannot be treated as a matched before/after CPU comparison. The
retained evidence does not establish whether any host music files were imported
or changed.

After three warmups, the MCP driver ran 20 local starts, 20 seeks at each of
1%, 50%, and 95%, and one paused-seek/resume check. All passed. Raw allowlisted
observations are in `local-playback-qualification.json`.

| Native diagnostic | Attempts | Median | p95 | Worst |
| --- | ---: | ---: | ---: | ---: |
| Session start to first mixer-consumed audio | 20 | 37 ms | 81 ms | 125 ms |
| Seek to first mixer-consumed audio | 60 | 59 ms | 87 ms | 92 ms |

These native measurements exclude the renderer Play click, MCP transport,
and command preprocessing before the session clock starts. Separate wall
observations are retained in the raw file. This generated
local-WAV workload does not qualify remote sources, recording correctness,
physical audibility, asset-heavy real libraries, release builds, other platforms,
or historical WebView comparisons. Existing applications and the shared Vite
server remained running.

The worst harness-observed start-to-first-audio wall time was 6,407 ms even
though the native first-audio diagnostic worst was 125 ms. That native timer
begins after app command preprocessing and excludes renderer, IPC and polling;
the larger wall observation remains part of the end-to-end result. The
accompanying profiler preflight failed to track the already-running Vite server.
No CPU/RSS/I/O samples or 30-minute memory qualification are claimed for this
run; `failure.txt` records that separate harness failure.
