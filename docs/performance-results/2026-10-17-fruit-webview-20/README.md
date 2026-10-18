# Twenty Fruit Bats WebView clock-progress requests

Linux debug native `7a43ea0c8def`, App entry `3943b23cfb37`, SearchPanel `980a63985977`. After the separate three fresh-profile exploratory passes, this batch ran 20 requests in one new isolated profile: one initial request and 19 warm repeats. Monochrome was the sole provider; fallback and experimental flags were disabled. Each request used a new queued UUID. Each repeat verified the previous cancelled operation and exact paused queue/media owner before proceeding. There was no setup playback.

All 20 requests resolved to `monochrome:239910648984977408`, retained one playback epoch, passed five contiguous healthy seconds with at least 4.5 seconds of media-clock advance, and verified owned pause, volume restoration, operation cancellation and final cleanup. No failed attempted case was dropped or retried with another UUID. The batch took 299.975 seconds.

| Measurement | Median | p95 | Worst |
| --- | ---: | ---: | ---: |
| Request to first observed attached source, all 20 | 1,457 ms | 1,575 ms | 3,214 ms |
| Same measurement, 19 warm repeats | 1,449 ms | 1,575 ms | 1,575 ms |
| Healthy window wall time | 5.363 s | 5.477 s | 5.569 s |
| Healthy window media-clock advance | 5.324 s | 5.457 s | 5.513 s |

Percentiles use nearest rank. These are observed source-attachment upper bounds with guarded MCP polling overhead, **not first-audio/mixer latency**. The source-attachment observation can precede the playing state. The 20 resolver ends correlate to the observed source generations: the first has candidate/preview cache misses and a probe, while the other 19 have both cache hits. Resolver elapsed median/p95/worst is 4/5/1,448 ms; that distribution is dominated by warm caches and must not be presented as cold startup.

The post-attachment sustained observations in seven cases (3, 9, 15–19) retained eight startup buffering-state samples, followed by seven buffering-to-playing transitions in their same epochs. Earlier resolving/buffering samples remain in the raw timelines and are excluded from that count. Across the full timelines, 39 buffering-state polls include normal pre-attachment startup. Their eventual healthy windows pass. No two-second clock stall was counted. In several inspected buffering samples, the media clock already advances with readyState 4 while the app still reports buffering; this is not proof of a decoder interruption or recovery of the earlier Rodio underflow. Polling can miss shorter transients.

## Observed resources

An independent one-second sampler observed the exact app and descendants for 300.046 seconds. It does not include Vite, the MCP client, profiler overhead or other applications. This window includes resolution, playback, pauses, cleanup and inter-request work, rather than steady playback alone.

- Observed CPU: 32.340 CPU-seconds; approximately 10.900 app and 21.440 WebKit CPU-seconds. Aggregate one-core CPU percentage median/p95/peak: 9.593/26.788/45.592.
- Summed process RSS median/p95/peak: 580,034,560 / 590,544,896 / 601,612,288 bytes. Shared pages can be double-counted; this is not PSS or a memory-leak conclusion.
- Maximum three observed concurrent processes: Burrata, WebKit web and WebKit network. Short-lived processes and their CPU between polls may be missed.
- Kernel storage counters: 151,552 bytes read and 319,647,744 bytes written. All sampled writes belong to the WebKit web process. Exact files/cause remain unidentified; these counters do not establish physical SSD traffic or audio-body cache writes. The profile's final size is about 34 MB.
- Network bytes, database waits and IPC sizes were not measured by this sampler. Its operation ID is a batch label, not inferred per-request IPC correlation.

No runtime fix is justified solely by these write totals. The direct WebView path attaches the resolved URL to the media element; native StreamDownload does not explain this run. File-level attribution is the next bounded storage investigation.

## Session and UI preservation

The isolated app and owned sink were stopped/removed. The verified normal launcher confirmed its mounted frontend, and the complete 30-entry queue objects, occurrence IDs/order, index, paused position, volume and repeat/shuffle settings match the immediately preceding snapshot exactly.

A read-only Spotify artist top-track check then visually reviewed the current dark UI at 1914×515 and 956×513. Controls are compact and wrap; no player overlap is visible, and both geometry inspections have no findings. The provider returned no artist portrait, so the header's placeholder is expected; track artwork is visible. Original window tiling/rectangle and the previous view were restored, with the full playback snapshot still unchanged. This covers the artist top-track state, not album/release controls, light theme or all loading/error states.

Private evidence lives under `.cache/qualification-fruit-unpinned-webview/measured20/`: hash-pinned runner and contract files, all 20 immutable reports/logs, resolver log, batch and measured summaries, compressed process samples and metadata, before/after snapshots, stop/sink ledgers and UI captures. The compressed samples were decompressed and checked against their original SHA-256.

This closes this bounded Linux debug WebView clock-progress batch. It does not qualify audible/PCM output, recording acoustics, first-audio p95, seeks/chapters/expiry, other providers, release builds, native underflow, the 30-minute memory requirement or other operating systems. Public release stays held.
