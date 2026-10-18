# Fruit Bats pinned-source regression

A public Monochrome search returned the exact studio recording (`USMRG1967001`, 181.4 seconds), and source inspection succeeded. Main-profile WebView playback returned a recording-unavailable message. The original observation loop assumed native generation fields; its 25-second outer elapsed value is not a playback failure latency measurement.

The shared resolver marked audio-search rows pinned but reconstructed linked candidates only for YouTube/SoundCloud. A selected Monochrome row therefore had no candidate and skipped search. The fix derives the candidate ID from the validated canonical track URL, retains matching provenance only when its ID/URL agree, and keeps access/container/duration protection in the existing resolver. A public redacted fixture covers this recording plus stale provenance and invalid URLs. The fixed source has not yet been live-qualified.

A separate pinned YouTube control played a 181.394-second studio recording, with an advancing playhead, in approximately 8.7 seconds. It was muted, paused afterward, and prior volume restored. This establishes that the current WebView audio path can play that source; it is one observation, not a startup percentile or first-mixer-consumption measurement. No playlist writes or saved-tag changes occurred.
