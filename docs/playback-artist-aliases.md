# Automatic playback artist aliases

Burrata reads verified aliases locally during ordinary source matching. New identity research starts only after normal source resolution fails, when an artist mismatch or an empty search with a credible identity anchor could benefit, and foreground attempts remain. Successful ordinary playback, pinned sources, and inaccessible streams do not trigger speculative alias work.

Verified identity evidence is retained across restarts. Thirty days is a refresh interval, not deletion. Favorites and genre edits preserve hydration evidence; recording identity edits invalidate affected bindings. Temporary HTTP errors preserve positive evidence. Definitive conflicts invalidate bindings; completed unsuccessful discovery is cached for a day.

## Evidence and matching

- Exact Spotify track hydration is stored separately from saved tags. Complete provider artist IDs can be linked to a unique MusicBrainz artist by exact URL relations, allowing one artist mapping to serve different recordings.
- MusicBrainz artist aliases and independently linked Wikidata labels are available across languages. Search hints and generated spelling guesses cannot establish playback identity. Shared identifiers such as Discogs IDs can connect authorities without a direct MusicBrainz/Wikidata link.
- Independent recording evidence can establish a recording-scoped binding using a known ISRC, complete recording metadata and complete artist credits. Beatport, Traxsource and local sources use this narrow path; it does not bypass catalog occurrence or Spotify hydration checks.
- Every credited performer must match. Matching does not enumerate a limited Cartesian product, so later performers and less common aliases remain reachable. Bands, members, guests and separate personas are not merged.
- Candidate search, fresh extraction and source recovery use the same verified context. Title, duration, version, ISRC conflicts, previews, accessibility and chapter protections remain enforced. Decoding establishes playable audio, not artist identity or an exact master.

## Resource bounds and diagnostics

Discovery shares the original playback deadline and remaining query/extraction slots. Identical callers share work while retaining independent cancellation; artist identity lookups share cache/locks across recordings. No library-wide alias sweep, login window or provider write is started.

Set `BURRATA_PLAYBACK_TRACE=1` before launch to capture local redacted phase records. Alias phases distinguish cache reads, caller waiting, shared discovery work, metadata queue/request time, bytes, status and candidate rechecks. Partial HTTP events survive cancellation. These overlapping durations must not be added together as total startup overhead.

Use [the offline report tool](../scripts/performance/alias_playback_report.md) for separate debug/release and cold/warm comparisons. `BURRATA_ARTIST_ALIAS_DISCOVERY=0` disables new discovery while retaining existing cache-only matching.

## Qualification boundary

A documented artist alias does not by itself prove that a provider track belongs to that artist. The Chalkias/Halkias public authorities agree on aliases and shared identifiers, but the reported Spotify recording still needs an independently verified provider/recording connection. Network failures and unresolved recordings must be reported as such.
