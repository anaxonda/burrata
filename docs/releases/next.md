# Burrata 0.1.9

Changes since v0.1.8.

- Optional complete waveform, BPM and key estimates for eligible finite streams. Imported and manually entered values remain unchanged until replacement is explicitly chosen.
- Wider waveform seeking with visible partial coverage, retained observations across pause/restart, hover times, and a compact playback bar option.
- Library/settings backups, optional verified audio and lyrics, scheduled copies with retention, and reviewed Merge/Replace restore with interrupted-restore recovery.
- Configurable local M3U mirroring for playlists, liked songs and retained history, preserving folder/section structure and protecting edited or deleted files.
- Clearer file/text/URL imports and playlist export, with privacy-safe sharing, preserved duplicates and visible recording conflicts.
- Clipping-aware Track and Album loudness normalization, separate from player volume. Unknown streams remain unchanged with an explanation; local analysis is optional and cancellable.
- Improved lyric source selection, local sidecar/embedded lyrics, saved timing adjustments, translation controls and retry with fallback.
- Optional song recognition within long recordings, retaining approximate detected positions.
- Playback-source recovery keeps recording identity, and buffering/download status remains visible in compact layouts. Track details include play history and library/playlist membership.

The audit also bounds search-cache memory and concurrent audio analysis, fixes read-only existing-audio linking, and keeps cancellation failures visible instead of reporting work as stopped. Unfiltered history pages now fetch only the requested page: an isolated 100,000-row SQL benchmark improved from a 360.6 ms median full traversal to 0.8 ms for a 50-row page. These are query measurements, not an end-to-end application benchmark.

Functional acceptance used isolated Linux native/WebView routes, synthetic fixtures and read-only local recordings. Physical-device and final installer/platform results are recorded separately. Full background stream analysis is limited to eligible finite sources; estimates can remain unavailable when confidence is insufficient. Live radio does not have a complete-track waveform.
