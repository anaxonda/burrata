# Burrata 0.1.27

Draft release notes. Final native, package, performance, live-provider, and physical audio qualification is pending.

- Recording matching now parses feature and collaboration credits consistently across search and resolution, preserves unknown credit completeness, and keeps exact and balanced restoration distinctions. Flexible matching treats release, performance, mix, and length permissions independently; optional shorter and longer duration limits are configured separately in seconds.
- Provider searches share bounded flight deadlines. Playback diagnostics report classified, bounded provider failures, while background acquisition and retries remain outside the playback-critical path.
- Album actions can search Soulseek using the catalog release title and primary artist credit. The action opens an album search and does not start downloads or change playlists.
- Track selection, virtualized scroll restoration, and playlist reordering handle duplicate occurrences and filtered order more reliably. Reorder failures are announced. Info, queue, and sidebar panels have clearer visual boundaries; playback information retains its state through startup updates.
- Less-used views load on demand while route restoration waits for the view to mount. Album and artist views preserve their instance state. Integrated library saving reconciles local album tracks, with updated radio and queue flows.

## Qualification status

- A scoped live check currently resolves and decodes 6 of 12 tracks from the Django release. This verifies resolver and decoder output only; it does not verify operating-system audio output or qualify the other six tracks. Other inspected candidates were rejected on identity evidence.
- Halka remains ambiguous in legacy captures. Metadata captures do not establish source identity or delivered audio.
- Physical Mac and Windows audio-output qualification is incomplete. Final native-suite, package, quiet-performance, live-provider, and release-artifact checks are still pending.
