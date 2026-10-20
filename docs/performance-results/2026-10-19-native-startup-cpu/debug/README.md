# Exploratory debug native startup CPU

Three single-trial, muted Linux GUI runs at runtime source `b851e3e` (documentation HEAD `1ae3bbf`) use one isolated profile and the same debug executable. Both direct-resolver gates are enabled. These are known YouTube IDs, not catalog discovery. The WAV control is local synthetic audio. Optimized-build comparison is still pending; these results do not establish release performance or a regression.

| Track | Native request → first mixer audio | Source ready | Startup CPU, 0–5s | Steady CPU, 15–20s |
| --- | ---: | ---: | ---: | ---: |
| Matthew, `uZGRATckoBM` | 3,120ms | 2,507ms | 2.307 CPU-seconds | 1.706 CPU-seconds |
| Pengus / Victory Lap, `kzrZXghhy1k` | 1,079ms | 890ms | 2.088 CPU-seconds | 1.803 CPU-seconds |
| Synthetic WAV | 36ms | 15ms | 2.107 CPU-seconds | 2.014 CPU-seconds |

Each trial reaches at least 15 seconds of native progression, remains at volume zero, and finishes paused. No physical audibility is claimed. The native diagnostic timers start inside the app; CPU windows start at the MCP client's command-send marker. Client-observed playing additionally includes IPC/polling delay. The first remote trial has a cold process/client relative to the next trial; this is not a paired or randomized latency comparison.

CPU is the sum of sampled root-app and descendant process deltas, including WebKit and FFmpeg, excluding the sampler, MCP client and external Vite server. No Node helper was observed in the app tree. One CPU-second per wall second means one fully occupied logical core. Actual sample intervals have a median near 159ms, despite requesting 100ms. Deltas at window boundaries are proportionally apportioned; observed interval peaks can include activity on either side of that boundary. Exited/short-lived children can be missed. RSS sums double-count shared pages. PSS is refreshed at most once per second and endpoint values can be stale. Storage counters are not network-byte measurements.

The 0–5s window includes both startup and whatever playback follows; the 15–20s window is steady playback. The first two seconds after pause include teardown and are not an idle baseline. Compilation was stopped during these three captures; later optimized compilation does not overlap them. Debug CPU is substantial even for the local control, so attributing all CPU to YouTube startup would be unsupported.

The [summary](summary.json) contains per-role CPU, process/PSS bounds, actual sampling intervals and first-audio diagnostics; the raw trial JSON and CPU JSONL are retained. [Provenance](provenance.json) retains the executable hash. [Manifest](manifest.json) hashes the data, scripts, screenshot and setup-failure record. Run `python3 summarize.py --directory . --output /tmp/cpu-summary.json` from this directory to reproduce the summary. The archived driver is the actual capture script and uses the project-root MCP launcher.

[Paused Library screenshot](paused-library.png), captured outside the performance windows, shows the three fixture rows and compact footer without overlap at 954×1036. This is a bounded visual check, not coverage of every view. An earlier fixture-only launch failed because its manually seeded settings omitted `autoPreview`; [the failure record](setup-failure.json) retains that correction. No test audio started in that failed launch and no user profile was changed.
