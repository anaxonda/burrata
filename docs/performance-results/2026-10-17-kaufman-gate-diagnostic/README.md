# Kaufman current-build candidate-gate diagnostic

A fresh isolated Linux debug WebView run on native `37c190bbf14a` and frontend `3943b23cfb37` completed all 19 exact catalog occurrences in 137.297 seconds. All ended unavailable; none reached extraction. Playback acceptance remains false. This is a matching/coverage diagnostic, not a startup or p95 benchmark.

The original 19 track records were verified before playback after only three explicit serialization defaults (empty local availability, null local path and empty manual-fields map). Four exact public MusicBrainz artist/release keys were warmed through app APIs. Their payloads were not all identical to the earlier run, so this is not a controlled causal comparison. The initial preflight wrongly required only four entries in entire cache namespaces, overlooking unrelated releases automatically hydrated by the app; it stopped before requesting playback. The corrected preflight checks the four exact keys and all 19 records. That failed assertion remains recorded.

| Provider | Candidates evaluated | Low confidence | Raw duration difference ≤5 s | Eligible |
| --- | ---: | ---: | ---: | ---: |
| Monochrome | 3 | 0 | 3 | 0 |
| YouTube | 65 | 65 | 10 | 0 |
| SoundCloud | 116 | 116 | 3 | 0 |

All 184 candidates failed the existing final gates. Two Monochrome candidates contain the requested title phrase, but none of its three candidates has an exact canonical or verified-alias artist-credit counter. Counters overlap and are independent descriptive evidence, not causal rejection reasons. Raw duration difference is not the effective threshold of every gate. No title-only or duration-only acceptance is justified.

There are 19 correlated resolution starts/ends, 57 provider search groups, no search boundary issues or malformed trace lines, and zero extraction starts. Exact accepted queue ownership and terminal exhaustion were verified; original volume was restored. A fresh paused snapshot was captured and the exact isolated launcher stopped, with no Burrata GUI remaining. The normal profile and its preserved 30-occurrence snapshot were not modified.

Separate read-only MusicBrainz recording lookups for the first three exact MBIDs returned HTTP 200, matching IDs/titles, empty ISRC arrays and empty URL-relation arrays. These provide no authoritative fallback source IDs for those three; they do not establish that no source exists elsewhere.

Source review separately identifies a possible full multiartist-credit join-phrase mismatch (for example provider artist arrays versus a `feat.` credit). A private fix is under review. This run does not demonstrate that issue caused these album failures; no production matching rules were changed for the diagnostic.

Private receipts and logs are under `.cache/qualification-resolution-terminal-trace/kaufman/`; raw exact-recording lookups and hashes are under `.tmp/kaufman-recording-lookup-2026-10-17/`. All unavailable outcomes and preflight failures are retained. No release or alternate-backend promotion follows from this result.
