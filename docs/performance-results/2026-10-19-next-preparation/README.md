# Native next-track preparation: three exploratory pairs

All six arms completed a natural queue transition, five seconds of advancing
native next-track playhead, and owned-process cleanup. This run provides no
evidence of a handoff benefit from the 45-second window. It has three
observations per arm, no p95 or significance claim, and no production-policy or
release promotion.

The [original successful report](exploratory-original.json) is preserved byte for
byte: 3,991,255 bytes, SHA-256
`c57ef84dda35a9f0697ca5438a1f4866d1afdabbca2c4bb548819206092a649d`.
[The derived summary](exploratory-summary.json) keeps every arm and pair, combined
process-tree resource calculations, and limitations. Native executable SHA-256:
`fdfe126c306f8157f239bb22d4228848a0e1ba7368938017140aa45a615e8c0c`.

Each arm launched one fresh isolated Linux debug app/profile. Its queue contained
a generated 90-second local WAV and the same explicit public YouTube source
`uZGRATckoBM`, independently recorded as 228 seconds. Each arm muted volume,
selected that queue once, sought the local control once to 30 seconds, then
observed the natural transition. Every arm selected format `140`. This fixes
source identity for the comparison; it does not establish acoustic equivalence
with the Spotify catalog version. Initial discovery before the setup seek may
warm the source, and caches outside the profile are not declared cold.

| Pair | Order | 30 s handoff estimate | 45 s handoff estimate | 45 minus 30 |
| --- | --- | ---: | ---: | ---: |
| 1 | 30, 45 | 79 ms | 213 ms | +134 ms |
| 2 | 45, 30 | 187 ms | 135 ms | −52 ms |
| 3 | 30, 45 | −41 ms | 288 ms | +329 ms |

The per-arm medians are 79 ms at 30 seconds and 213 ms at 45 seconds. These are
100 ms MCP polling/forecast estimates relative to the local control's predicted
end. The −41 ms value is retained as forecast/poll noise, not evidence of audible
overlap or gaplessness. None of these values measures the speaker waveform or
sample-level first audio.

Preparation actually began with an estimated 29.881–30.102 seconds remaining for
the 30-second arms and 44.773–45.236 seconds for the 45-second arms. Median reader
queue age at adoption was 21.720 and 36.227 seconds, respectively. Starting
earlier therefore held the queued reader about 15 seconds longer here. Queue
age is not signed-URL age or remaining URL lifetime. URL expiry and network wire
bytes were not measured, and this run did not stress expiry, cancellation or
queue reordering.

## Observed combined process tree

The component labels in the original report are **unreliable**. A read-only host
snapshot found a direct child with `comm=MainThread` but executable basename
`node`, using 244,346,880 bytes of RSS. The old comm-only rule counted it as
`app`. The [separate process observation](process-attribution-observation.json)
retains only names, executable basenames, counts and resource counters, with no
PIDs, arguments or retained environment values. It identifies Node, not its exact
script. The historical native-app-only and helper-only peaks cannot be recovered
from the aggregate-only samples.

Combined sampled tree metrics remain usable within the sampling limitations.
For each timestamp, all component CPU/RSS values were summed first; maxima were
then taken across those simultaneous totals. Separately timed component maxima
were not added together. Recomputed CPU totals and RSS peaks match the stored
whole-tree counters for all six arms.

| Arm | Window | Sampled tree CPU | Whole-window CPU peak | Preparation CPU peak | Summed RSS peak |
| --- | ---: | ---: | ---: | ---: | ---: |
| 1 | 30 s | 27.410 CPU-s | 293.5% | 221.2% | 1,226.4 MiB |
| 2 | 45 s | 29.730 CPU-s | 251.7% | 251.7% | 1,254.6 MiB |
| 3 | 45 s | 27.220 CPU-s | 242.1% | 242.1% | 1,202.2 MiB |
| 4 | 30 s | 28.370 CPU-s | 252.3% | 252.3% | 1,242.6 MiB |
| 5 | 30 s | 27.860 CPU-s | 246.5% | 246.5% | 1,285.2 MiB |
| 6 | 45 s | 29.470 CPU-s | 246.8% | 246.8% | 1,252.2 MiB |

CPU 100% means one logical core. Each resource window spans about 66.4–66.7
seconds, including control setup, control playback, preparation and sustained
next playback. It excludes app/Vite launch time, Vite itself, the MCP observer
and external applications. The configured 200 ms sleep produced median observed
sample intervals of 244–247 ms. Short-lived or reparented helpers may be missed;
the first sighting of a process establishes a CPU baseline. Summed RSS may
double-count shared pages and is neither PSS nor evidence of a native-app memory
leak. The different lengths of the preparation phases also prevent interpreting
their CPU totals as equal-duration steady-state comparisons.

The three paired whole-window CPU differences for 45 minus 30 seconds are
+2.320, −1.150 and +1.610 CPU-seconds. This small exploratory sample does not
establish a repeatable CPU advantage for either window. The observed spikes are
real sampled tree activity; this archive cannot assign them to the native
process versus the misclassified helper.

No classifier fix or rerun was performed during this archival step. The current
GUI was not queried or changed.

## Earlier setup-only invocation

The first exploratory invocation did not reach playback. All six planned arms
were rejected by the launcher native-source freshness guard; cleanup completed.
The original driver reported only `launcher_failed` and removed its disposable
logs. A later read-only execution of the launcher prefix reproduced the
stale-binary rejection before any Vite/app launch. That binary predated five
changed Rust sources, including production files and test-only research modules.

The [original setup-failure report](initial-stale-build-failure.json) remains
unchanged, SHA-256
`bd15dc680cdd44035479d075e9438e27f685b2143c0f49baca319b3e60d3e66b`.
Those are setup failures, not source, transition or CPU measurements. The harness
was then changed to retain typed setup reasons and partial evidence and stop
with remaining arms `not_run`. A fresh executable was built for the separate
successful run; the launcher gate was retained.
