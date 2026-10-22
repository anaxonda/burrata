# Saved Bacc paired cold/warm resolver cohort

Frozen diagnostic SHA256: `c12735ca72220335d9474270c828e4fe96eb37cd72fb059f4ea8c9cb91a84ea4`. Public settings prefer Monochrome with YouTube/SoundCloud fallbacks, 1500 ms fallback grace, 20-second startup budget; no normal-profile accounts or library were used. The diagnostic uses the production foreground resolver context and five-second FFmpeg PCM decode, not the GUI mixer or physical output. Native snapshot is the header/metadata build qualified by 1583 tests; the later BPM generation-label change and later UI controls are outside this artifact.

Three warmup pairs were excluded, followed by twenty measured cold/warm pairs. Cold means a new disposable Store/AcquisitionManager and stopped provider helper; OS/tool caches are not flushed. Warm immediately reuses its paired store/manager. Every measured attempt succeeded, and all pairs retained `youtube:rpCswezRrLs` (zero source changes).

| Measurement | Cold median | Cold p95 | Warm median | Warm p95 |
|---|---:|---:|---:|---:|
| Resolve | 9691 ms | 11011 ms | 487 ms | 1507 ms |
| Request to first decoded PCM | 9995 ms | 11388 ms | 705 ms | 1679 ms |

Each p95 has twenty timing samples; failures remain in the denominator (none occurred in this case). This is one recording, one mode, one machine/network observation. It demonstrates warm reuse in this build, not improvement versus a pre-change baseline or accuracy/latency across catalogs.

The diagnostic records CPU deltas and child resource counters. Reaped-child maximum RSS is a cumulative process high-water value (approximately 355 MB here), so it must not be interpreted as independent per-attempt memory, especially for warm attempts. Logical phase/HTTP request counts were not collected in this run; none are inferred from CPU or elapsed time.

The sanitized aggregate is [balanced-bacc-cold-warm-cohort.json](balanced-bacc-cold-warm-cohort.json). Owner-readable raw evidence remains under `.cache/matching-stage5-balanced-cohort-20261021/`; raw URLs/headers are not published.
