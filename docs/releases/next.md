# Burrata 0.1.28

Draft release notes. Full native-suite, cross-platform package, installed-artifact, live-provider, and physical audio-output qualification is pending.

- Recording identity now stays consistent when catalog tracks are read raw, hydrated into the Library, and queued for playback. Roleless catalog artist credits remain usable through hydration, `Primary Artist` is recognized consistently, and supporting credits such as `Vocals` alone do not become the recording's main artist. Malformed or oversized recovered credits are rejected, and placeholder artists do not generate precision fallback searches.
- Playback provider work shares bounded operation deadlines. Provider attempts retain bounded phase, failure category, retryability, and timing diagnostics; malformed all-invalid result pages are reported as errors instead of cached as successful empty searches. A timeout remains a provider or transport outcome, not proof that a recording is unavailable.
- Track lists preserve global row identity through sliced and virtualized views, including duplicate occurrences. Focus, highlighting, playback selection, and reorder error announcements follow the displayed occurrence.
- Radio rows keep their recording identity across queued and persisted playback state. Favorite actions distinguish unresolved candidates from verified matches, and radio selection does not substitute a different recording when metadata is incomplete.
- Library album and artist views keep their grid/list preferences independently. Playback information retains its state through startup updates, and fullscreen artwork and player controls have improved spacing and fit.

## Qualification status

- The Linux installed-artifact, live-provider, and private audio-sink runs are pending the final candidate build. A successful resolver or decoder check will qualify only its exact recording, provider, and captured PCM window; it will not establish physical-device output.
- Public provider availability remains subject to transport, helper readiness, extraction, region, and rate limits. A timeout is not evidence that a track is absent, and no provider or network cause is claimed resolved by this release.
- Legacy Library rows can retain ambiguous identity when saved metadata does not contain sufficient verified catalog evidence. Playback continues to fail closed where identity cannot be established; this release does not rewrite those records.
- No cleanup or migration has been performed against the normal Library database or profile.
