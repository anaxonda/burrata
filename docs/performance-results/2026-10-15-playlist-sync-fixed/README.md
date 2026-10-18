# Playlist sync idle-capture comparison

One paired 120-second development-build capture on the main profile compared the baseline with the fixed build, both while playback was idle. App CPU fell from 13.630 s to 7.590 s (44.3%); p95 app CPU per sample fell from 100.313% to 59.594% of one core. Median sample CPU was effectively unchanged (1.742% to 1.739%). The binaries differ, and the baseline phase log covers time since launch, including HMR, so its phase observations are not exactly aligned with the CPU window. Treat this as one observed paired capture, not a general performance guarantee.

The fixed build recorded two healthy watcher checks and two successful sync passes. Each pass reconciled 36 mirrored collections in about 2.7–2.8 seconds; mirror work dominated the roughly 2.9–3.0 second pass. Native startup analysis reported zero native analysis work and zero facts, ruling those out as the source of the measured startup profile in this capture.

The optimization keeps the explicit sync fallback when the watcher is missing, stopped, or reports an error. Manual sync, startup sync, import sync, and settings-triggered sync remain unchanged. This evidence does not establish behavior for every target, release-build performance, a Rust cutover, or leak freedom.

Raw reports: [fixed summary](summary.json), [fixed phase observations](phase-observations.json), [fixed profile](profile/samples.json), [baseline summary](../2026-10-15-playlist-sync-baseline/summary.json), [baseline phase observations](../2026-10-15-playlist-sync-baseline/phase-lines-since-launch.json), and [baseline profile](../2026-10-15-playlist-sync-baseline/profile/samples.json).
